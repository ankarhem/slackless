//! Front-end-agnostic Slack domain logic shared by GUI and CLI.
//!
//! Pure parsers, serializers, formatters, and cross-platform helpers live here so front-ends stay
//! thin and never diverge on Slack protocol rules, export formats, or update verification.

pub mod calling;
pub mod canvas;
pub mod canvas_doc;
pub mod huddles;
pub mod lists;
pub mod mrkdwn;
pub mod slash;
pub mod updater;
pub mod webvtt;
