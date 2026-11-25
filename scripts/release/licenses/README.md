License texts that the release AppImage needs and its sources do not give
(see licenses.py):

- `libxcb/COPYING` — from libxcb 1.13.1, AlmaLinux's source package
  `libxcb-1.13.1-9.el9.src.rpm`: no installed package carries it.
- `xcb-util-keysyms/COPYING` — the MIT notice at the top of `keysyms.c` of
  xcb-util-keysyms 0.4.0 (`xcb-util-keysyms-0.4.0-17.el9.src.rpm`), which
  ships no license file.
- `Apache-2.0.txt` — the Apache License 2.0 as apache.org gives it, for
  Rust crates under Apache-2.0 (or a choice that includes it) whose
  packages carry no license file.
