"""orbis's plugin for yt-dlp.

orbis writes this file out under a private folder and hands that folder to
yt-dlp with `--plugin-dirs`, together with `--cache-dir` in orbis's own cache.

It does two things:

- It turns on yt-dlp's cache of the preprocessed player JS. Without it,
  Node.js parses YouTube's player anew for every track (~1.1 s); with it,
  ~0.4 s. yt-dlp keeps the cache off because it never removes old players
  (~4.4 MB each); orbis removes them itself (see `ytdlp.rs`). When a newer
  yt-dlp no longer has the switch, nothing happens and links take as long as
  before.
- It adds the address `orbis-serve:`, which keeps yt-dlp running and answers
  requests for links (the worker, see below and `worker.rs`).
"""

import concurrent.futures
import hashlib
import io
import json
import os
import sys
import threading

from yt_dlp.extractor.common import InfoExtractor

try:
    from yt_dlp.extractor.youtube.jsc._builtin import ejs as _ejs
except ImportError:
    _ejs = None

_provider = getattr(_ejs, 'EJSBaseJCP', None)
if hasattr(_provider, '_ENABLE_PREPROCESSED_PLAYER_CACHE'):
    _provider._ENABLE_PREPROCESSED_PLAYER_CACHE = True


# ---- the worker -------------------------------------------------------------
#
# `yt-dlp --ignore-config --quiet --simulate --plugin-dirs <dir> orbis-serve:`
# runs `OrbisServeIE`, which answers requests for links until stdin closes
# (see `worker.rs`). It first writes {"ready": true, "version": "…"}. A
# request is a line of JSON:
#
#     {"n": 1, "video_id": "…", "format": "774/251/…", "quick": true,
#      "cookies": "<Netscape cookies>" or null, "js_runtime": "node" or null,
#      "cache_dir": "…" or null}
#
# and its answer is a line {"n": 1, "info": {…}} with the fields of the chosen
# format that orbis reads, or {"n": 1, "error": "WARNING: …\nERROR: …"}: the
# request's warnings, then its error, as yt-dlp's stderr would have them.
# Answers come in the order they are ready. Requests run three at a time,
# each thread with YoutubeDL objects of its own. The cookies are read from
# memory and never written to a file.

_THREADS = 3
# The last warnings of a request that go back with its error.
_WARNINGS = 10
# The fields of a format that `parse_info` in `ytdlp.rs` reads.
_FORMAT_KEYS = ('url', 'format_id', 'acodec', 'ext', 'abr', 'tbr', 'asr', 'http_headers')
# The arguments of the quick way (`QUICK_ARGS` in `ytdlp.rs`).
_QUICK = {'youtube': {'player_client': ['web_music'], 'player_skip': ['configs']}}

_local = threading.local()
_out = threading.Lock()


def _send(answer):
    line = json.dumps(answer, separators=(',', ':'))
    with _out:
        sys.stdout.write(line + '\n')
        sys.stdout.flush()


class _Logger:
    """Keeps yt-dlp's messages off stdout, which carries the answers. The
    warnings of the request at hand are kept: a failed answer brings them
    back, and orbis reads in them why it failed (such as a challenge solver
    too old for YouTube's player). A YoutubeDL, and with it its logger,
    serves one thread."""

    def __init__(self):
        self.warnings = []

    def debug(self, message):
        pass

    info = error = debug

    def warning(self, message):
        self.warnings.append(message)


def _youtube_dl(request):
    """A YoutubeDL for the request's cookies and options, made once a thread."""
    import yt_dlp

    cookies = request.get('cookies')
    key = (
        hashlib.sha256(cookies.encode()).hexdigest() if cookies else None,
        request.get('format'), bool(request.get('quick')),
        request.get('js_runtime'), request.get('cache_dir'),
    )
    made = getattr(_local, 'made', None)
    if made is None:
        made = _local.made = {}
    ydl = made.get(key)
    if ydl is not None:
        return ydl
    params = {
        'quiet': True, 'no_warnings': True, 'noprogress': True, 'noplaylist': True,
        'logger': _Logger(), 'format': request.get('format') or 'bestaudio',
    }
    if request.get('quick'):
        params['extractor_args'] = _QUICK
    if request.get('js_runtime'):
        params['js_runtimes'] = {request['js_runtime']: {'path': None}}
    if request.get('cache_dir'):
        params['cachedir'] = request['cache_dir']
    if cookies:
        params['cookiefile'] = io.StringIO(cookies)
    ydl = yt_dlp.YoutubeDL(params)
    if cookies:
        ydl.cookiejar  # loaded now, from memory
        ydl.params['cookiefile'] = None  # and never written back
    # Other cookies or another quality: the old objects are not needed.
    if len(made) >= 2:
        for old in made.values():
            old.close()
        made.clear()
    made[key] = ydl
    return ydl


def _slim(info):
    def fields(format_):
        return {key: format_[key] for key in _FORMAT_KEYS if format_.get(key) is not None}

    slim = fields(info)
    if info.get('duration') is not None:
        slim['duration'] = info['duration']
    if info.get('requested_formats'):
        slim['requested_formats'] = [fields(f) for f in info['requested_formats']]
    return slim


def _answer(request):
    n = request.get('n')
    logger = None
    try:
        ydl = _youtube_dl(request)
        logger = ydl.params['logger']
        logger.warnings.clear()
        info = ydl.extract_info(
            f'https://music.youtube.com/watch?v={request["video_id"]}', download=False)
        _send({'n': n, 'info': _slim(info)})
    except Exception as error:
        warnings = logger.warnings[-_WARNINGS:] if logger is not None else []
        lines = [f'WARNING: {warning}' for warning in warnings]
        lines.append(str(error) or type(error).__name__)
        _send({'n': n, 'error': '\n'.join(lines)})


def _serve():
    from yt_dlp.version import __version__

    _send({'ready': True, 'version': __version__})
    pool = concurrent.futures.ThreadPoolExecutor(_THREADS)
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        if isinstance(request, dict) and request.get('video_id'):
            pool.submit(_answer, request)
    # Stdin closed: orbis stopped the worker or quit. Requests in flight
    # have nobody to answer to.
    os._exit(0)


class OrbisServeIE(InfoExtractor):
    IE_NAME = 'orbis:serve'
    IE_DESC = False
    _VALID_URL = r'orbis-serve:$'

    def _real_extract(self, url):
        _serve()
