//! Front-end-agnostic Slack domain logic shared by GUI and CLI.
//!
//! Pure parsers, serializers, formatters, and cross-platform helpers live here so front-ends stay
//! thin and never diverge on Slack protocol rules, export formats, or update verification.

pub mod canvas;
pub mod huddles;
pub mod lists;
pub mod slash;
pub mod updater;
pub mod webvtt;
