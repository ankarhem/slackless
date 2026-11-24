//! Pure drawing math, Catmull-Rom cubic stroke smoothing, fade curves, and reaction glyph helpers.

use super::*;

pub(super) fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Compute `(alpha, width_scale, needs_anim_frame)` for a fading huddle draw stroke.
/// Active strokes stay bright at `1.0`; completed strokes hold for 3.8s then smoothly ease out
/// over 1.6s at display refresh rate.
pub(super) fn stroke_fade_curve(ended_at_ms: Option<u64>, now_ms: u64) -> (f32, f32, bool) {
    const HOLD_MS: f32 = 3_800.0;
    const FADE_MS: f32 = 1_600.0;
    let Some(ended_at) = ended_at_ms else {
        return (1.0, 1.0, false);
    };
    let elapsed = now_ms.saturating_sub(ended_at) as f32;
    if elapsed < HOLD_MS {
        // Request animation frames once within the last 100ms of hold so the fade starts smoothly.
        return (1.0, 1.0, elapsed >= HOLD_MS - 100.0);
    }
    if elapsed >= HOLD_MS + FADE_MS {
        return (0.0, 0.72, false);
    }
    let t = ((elapsed - HOLD_MS) / FADE_MS).clamp(0.0, 1.0);
    // Smooth cubic ease-out (`(1 - t)^2` * `(1 + 2t)` complement -> smoothstep).
    let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
    let width_scale = 0.72 + 0.28 * smooth;
    (smooth, width_scale, true)
}

/// Fit a $C^1$-smooth Catmull-Rom cubic Bézier segment from `p1` to `p2` using neighbors `p0`/`p3`,
/// clamping control handles to `0.45 * chord` so sharp bends never self-intersect.
pub(super) fn make_catmull_rom_segment(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) -> slackless_core::huddle::HuddleDrawSegment {
    let chord_dx = p2.0 - p1.0;
    let chord_dy = p2.1 - p1.1;
    let chord_len = (chord_dx * chord_dx + chord_dy * chord_dy).sqrt();
    if chord_len < 1e-9 {
        return slackless_core::huddle::HuddleDrawSegment {
            a: [p1.0, p1.1],
            b: [p1.0, p1.1],
            c: [p2.0, p2.1],
            d: [p2.0, p2.1],
        };
    }
    let max_handle = chord_len * 0.45;
    let clamp_vec = |vx: f64, vy: f64| -> (f64, f64) {
        let len = (vx * vx + vy * vy).sqrt();
        if len > max_handle && len > 1e-9 {
            let s = max_handle / len;
            (vx * s, vy * s)
        } else {
            (vx, vy)
        }
    };
    let (h1x, h1y) = clamp_vec((p2.0 - p0.0) / 6.0, (p2.1 - p0.1) / 6.0);
    let (h2x, h2y) = clamp_vec((p3.0 - p1.0) / 6.0, (p3.1 - p1.1) / 6.0);
    slackless_core::huddle::HuddleDrawSegment {
        a: [p1.0, p1.1],
        b: [p1.0 + h1x, p1.1 + h1y],
        c: [p2.0 - h2x, p2.1 - h2y],
        d: [p2.0, p2.1],
    }
}

pub(super) fn build_smooth_circle(cx: f32, cy: f32, radius: f32) -> PathBuilder {
    const N: usize = 16;
    let mut pb = PathBuilder::fill();
    for i in 0..N {
        let ang = (i as f32 / N as f32) * std::f32::consts::TAU;
        let p = point(px(cx + radius * ang.cos()), px(cy + radius * ang.sin()));
        if i == 0 {
            pb.move_to(p);
        } else {
            pb.line_to(p);
        }
    }
    pb.close();
    pb
}

/// Build an outward-only semicircular end cap centered at `(cx, cy)` extending in unit direction
/// `(dx, dy)` so it meets a butt-capped stroke edge with zero overlap.
pub(super) fn build_outward_cap(cx: f32, cy: f32, dx: f32, dy: f32, radius: f32) -> PathBuilder {
    const STEPS: usize = 8;
    let base_ang = dy.atan2(dx) - std::f32::consts::FRAC_PI_2;
    let mut pb = PathBuilder::fill();
    for s in 0..=STEPS {
        let ang = base_ang + (s as f32 / STEPS as f32) * std::f32::consts::PI;
        let p = point(px(cx + radius * ang.cos()), px(cy + radius * ang.sin()));
        if s == 0 {
            pb.move_to(p);
        } else {
            pb.line_to(p);
        }
    }
    pb.close();
    pb
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_smooth_stroke(
    window: &mut Window,
    segments: &[slackless_core::huddle::HuddleDrawSegment],
    hue: i32,
    alpha: f32,
    width_scale: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) {
    use gpui_kit::gpui::{PathStyle, StrokeOptions};

    if segments.is_empty() {
        return;
    }
    let to_xy = |p: [f64; 2]| (left + p[0] as f32 * width, top + p[1] as f32 * height);
    let h = (hue.rem_euclid(360)) as f32 / 360.0;
    let color = hsla(h, 0.90, 0.54, 0.96 * alpha);
    let stroke_w = 5.0 * width_scale;
    let cap_r = stroke_w * 0.5;

    // Split into contiguous sub-chains so disjoint segments never bridge with a stray line.
    let mut chains: Vec<&[slackless_core::huddle::HuddleDrawSegment]> = Vec::new();
    let mut start = 0usize;
    for i in 1..segments.len() {
        let prev_d = to_xy(segments[i - 1].d);
        let curr_a = to_xy(segments[i].a);
        if (curr_a.0 - prev_d.0).hypot(curr_a.1 - prev_d.1) > 4.0 {
            chains.push(&segments[start..i]);
            start = i;
        }
    }
    chains.push(&segments[start..]);

    for chain in chains {
        // Single-click dot or degenerate zero-length stroke.
        let first_a = to_xy(chain[0].a);
        let total_chord: f32 = chain
            .iter()
            .map(|s| {
                let a = to_xy(s.a);
                let d = to_xy(s.d);
                (d.0 - a.0).hypot(d.1 - a.1)
            })
            .sum();
        if total_chord < 1.0 {
            if let Ok(dot) = build_smooth_circle(first_a.0, first_a.1, cap_r).build() {
                window.paint_path(dot, color);
            }
            continue;
        }

        // Use Lyon's `StrokeTessellator` on the true cubic Bézier curve (never punches holes on
        // self-crossing loops like cursive letters or figure-8s!) with clamped miter limit and
        // clean outward semicircular end caps.
        let opts = StrokeOptions::default()
            .with_line_width(stroke_w)
            .with_miter_limit(1.5)
            .with_tolerance(0.15);
        let mut pb = PathBuilder::default().with_style(PathStyle::Stroke(opts));
        for (ix, seg) in chain.iter().enumerate() {
            let a = to_xy(seg.a);
            let b = to_xy(seg.b);
            let c = to_xy(seg.c);
            let d = to_xy(seg.d);
            if ix == 0 {
                pb.move_to(point(px(a.0), px(a.1)));
            }
            pb.cubic_bezier_to(
                point(px(d.0), px(d.1)),
                point(px(b.0), px(b.1)),
                point(px(c.0), px(c.1)),
            );
        }
        if let Ok(built) = pb.build() {
            window.paint_path(built, color);
        }

        // Start & end outward semicircular caps (strictly outside stroke butt edges -> zero seam).
        let s0 = chain[0];
        let a0 = to_xy(s0.a);
        let t0 = {
            let b0 = to_xy(s0.b);
            let d0 = to_xy(s0.d);
            let (dx, dy) = if (b0.0 - a0.0).hypot(b0.1 - a0.1) > 0.2 {
                (a0.0 - b0.0, a0.1 - b0.1)
            } else {
                (a0.0 - d0.0, a0.1 - d0.1)
            };
            let l = dx.hypot(dy).max(1e-5);
            (dx / l, dy / l)
        };
        if let Ok(cap) = build_outward_cap(a0.0, a0.1, t0.0, t0.1, cap_r).build() {
            window.paint_path(cap, color);
        }
        let sl = chain[chain.len() - 1];
        let dl = to_xy(sl.d);
        let tl = {
            let cl = to_xy(sl.c);
            let al = to_xy(sl.a);
            let (dx, dy) = if (dl.0 - cl.0).hypot(dl.1 - cl.1) > 0.2 {
                (dl.0 - cl.0, dl.1 - cl.1)
            } else {
                (dl.0 - al.0, dl.1 - al.1)
            };
            let l = dx.hypot(dy).max(1e-5);
            (dx / l, dy / l)
        };
        if let Ok(cap) = build_outward_cap(dl.0, dl.1, tl.0, tl.1, cap_r).build() {
            window.paint_path(cap, color);
        }
    }
}

pub(super) fn sticky_emoji_glyph(code: &str) -> &str {
    match code {
        "raised_hand" => "✋",
        "coffee" => "☕",
        "headphones" => "🎧",
        "white_check_mark" => "✅",
        "+1" | "thumbsup" => "👍",
        "tada" => "🎉",
        "heart" => "❤️",
        "joy" => "😂",
        "clap" => "👏",
        "fire" => "🔥",
        "eyes" => "👀",
        "raised_hands" => "🙌",
        "confetti" => "🎉",
        "hearts" => "💖",
        "sparkles" => "✨",
        "rocket" => "🚀",
        "mic_drop" => "🎤",
        "trophy" => "🏆",
        "unicorn" => "🦄",
        other => other,
    }
}

pub(super) fn effect_label(code: &str) -> (&'static str, &'static str) {
    match code {
        "confetti" => ("🎉", "Confetti"),
        "hearts" => ("💖", "Hearts"),
        "clap" => ("👏", "Applause"),
        "sparkles" => ("✨", "Sparkles"),
        "rocket" => ("🚀", "Rocket"),
        "mic_drop" => ("🎤", "Mic Drop"),
        "trophy" => ("🏆", "Trophy"),
        "unicorn" => ("🦄", "Unicorn"),
        _ => ("✨", "Effect"),
    }
}
