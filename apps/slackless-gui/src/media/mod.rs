//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. Downloads, cached inline audio/video playback, and animated [`Picture`] rendering
//! live in focused submodules and are re-exported here.

mod audio_player;
mod downloads;
mod http;
mod picture;

pub use audio_player::{audio_state, play_video, stop_audio, toggle_audio, video_loading};
pub use downloads::{
    DownloadStatus, clear_finished_downloads, download, download_as, downloads, enable_downloads,
    fetch, open_local_file, reveal_in_finder, set_ask_download_location, set_download_dir,
};
pub use http::CoreHttp;
pub use picture::Picture;
