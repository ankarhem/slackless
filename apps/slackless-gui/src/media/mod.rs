//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. Downloads, cached inline audio/video playback, and animated [`Picture`] rendering
//! live in focused submodules and are re-exported here.

mod audio_player;
mod downloads;
mod http;
mod picture;
mod video_player;

pub(crate) use audio_player::fetch_cached_media;
pub use audio_player::{audio_state, stop_audio, toggle_audio};
pub(crate) use downloads::active_fetcher;
pub use downloads::{
    DownloadStatus, clear_finished_downloads, download, download_as, downloads, enable_downloads,
    fetch, open_local_file, reveal_in_finder, set_ask_download_location, set_download_dir,
};
pub use http::CoreHttp;
pub use picture::Picture;
pub(crate) use video_player::{
    VIDEO_FRAME_INTERVAL, copy_player_surface, take_inline_handoff_for_viewer,
};
pub use video_player::{
    VideoSurface, format_video_time, inline_video_snapshot, play_inline_video, progress_fraction,
    render_video_surface, sanitize_secs, seek_inline_video, stop_inline_video,
    stop_inline_video_for_expand, toggle_inline_video_mute, toggle_inline_video_pause,
    video_scrub_bar,
};
