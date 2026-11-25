//! `orbis-cli` commands that change the account: ratings and playlist edits.

use std::error::Error;

use ytm::{Privacy, Rating};

use crate::with_client;

pub async fn rate(video_id: &str, rating: Rating) -> Result<(), Box<dyn Error>> {
    with_client(|client| async move { client.rate(video_id, rating).await }).await?;
    println!("{video_id}: {rating:?}");
    Ok(())
}

pub async fn rating(video_id: &str) -> Result<(), Box<dyn Error>> {
    let queue = with_client(|client| async move { client.watch(video_id, None).await }).await?;
    println!("{video_id}: {:?}", queue.current_like);
    Ok(())
}

pub async fn options(video_id: &str) -> Result<(), Box<dyn Error>> {
    let options =
        with_client(|client| async move { client.playlist_options(video_id).await }).await?;
    for option in &options {
        println!(
            "- {} — {} [{}]",
            option.title, option.subtitle, option.playlist_id
        );
    }
    Ok(())
}

pub async fn add(video_id: &str, playlist_id: &str) -> Result<(), Box<dyn Error>> {
    let set_video_id =
        with_client(|client| async move { client.add_to_playlist(playlist_id, video_id).await })
            .await?;
    println!("added {video_id} to {playlist_id} (entry {set_video_id:?})");
    Ok(())
}

pub async fn remove(video_id: &str, playlist_id: &str) -> Result<(), Box<dyn Error>> {
    if playlist_id == ytm::LIKED_MUSIC {
        with_client(|client| async move { client.rate(video_id, Rating::None).await }).await?;
        println!("removed {video_id} from Liked Music (like taken back)");
        return Ok(());
    }
    let removed = with_client(|client| async move {
        let playlist = client.playlist_all(playlist_id).await?;
        let entries: Vec<String> = playlist
            .tracks
            .iter()
            .filter(|t| t.video_id == video_id)
            .filter_map(|t| t.set_video_id.clone())
            .collect();
        for entry in &entries {
            client
                .remove_from_playlist(playlist_id, video_id, entry)
                .await?;
        }
        Ok(entries.len())
    })
    .await?;
    if removed == 0 {
        return Err(format!("{video_id} is not in {playlist_id}").into());
    }
    println!("removed {removed} entries of {video_id} from {playlist_id}");
    Ok(())
}

pub async fn create(title: &str, description: &str) -> Result<(), Box<dyn Error>> {
    let id = with_client(|client| async move {
        client
            .create_playlist(title, description, Privacy::Private, &[])
            .await
    })
    .await?;
    println!("{id}");
    Ok(())
}

pub async fn delete(playlist_id: &str) -> Result<(), Box<dyn Error>> {
    with_client(|client| async move { client.delete_playlist(playlist_id).await }).await?;
    println!("deleted {playlist_id}");
    Ok(())
}
