//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. Downloads, cached inline audio/video playback, and animated [`Picture`] rendering
//! live in focused submodules and are re-exported here.

mod audio_player;
mod downloads;
mod http;
mod pdf;
mod picture;
mod video_player;

pub(crate) use audio_player::fetch_cached_media;
pub use audio_player::{
    audio_state, cycle_audio_rate, seek_audio, stop_audio, toggle_audio, toggle_audio_mute,
};
pub(crate) use downloads::active_fetcher;
pub use downloads::{
    DownloadStatus, clear_finished_downloads, download, download_as, downloads, enable_downloads,
    fetch, open_local_file, reveal_in_finder, set_ask_download_location, set_download_dir,
};
pub use http::CoreHttp;
pub(crate) use pdf::fetch_and_render_pdf_page;
pub use pdf::{
    cached_pdf_inline_thumb, cached_pdf_page, cached_pdf_page_count, request_pdf_inline_thumb,
    store_cached_pdf_page,
};
pub use picture::Picture;
#[cfg(test)]
pub(crate) use video_player::seed_inline_video_for_test;
pub use video_player::{
    InlineVideoMeta, VideoSurface, active_pip_video, cycle_inline_video_rate, finish_transcription,
    format_playback_rate, format_video_time, inline_video_begin_frame, inline_video_snapshot,
    install_transcription_dispatcher, is_hls_url, is_transcribing, mark_inline_video_rendered,
    play_inline_video, play_inline_video_with_meta, progress_fraction, render_video_surface,
    sanitize_secs, seek_inline_video, stop_active_inline_video, stop_inline_video,
    stop_inline_video_for_expand, toggle_inline_video_mute, toggle_inline_video_pause,
    transcribe_media, update_inline_video_viewport_visibility, video_scrub_bar,
};
pub(crate) use video_player::{
    VIDEO_FRAME_INTERVAL, copy_player_surface, take_inline_handoff_for_viewer,
};
