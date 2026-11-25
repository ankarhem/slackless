//! Floating message hover toolbar and unified [`HoverOverlay`] / [`HoverHandle`] lifecycle.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, h_flex};
use gpui_kit::gpui::{
    Anchor, AnyElement, App, Div, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, px,
};

use super::menu::{MenuOrigin, actions_menu};
use super::{ActionHandler, ActionSet, MessageAction};
use crate::emoji;

/// Slack's hover-bar buttons: 32px squares.
const BAR_BUTTON: Size = Size::Medium;

fn toolbar_button(id: String, handler: &ActionHandler, action: MessageAction) -> Button {
    let handler = handler.clone();
    Button::new(SharedString::from(id))
        .ghost()
        .with_size(BAR_BUTTON)
        .on_click(move |_, window, cx| handler(action.clone(), window, cx))
}

/// Shared hover/open state linking a host row (e.g. a message or staged GIF card) with its
/// overhanging floating interaction overlay (e.g. `hover_toolbar` at `top: -16px`).
#[derive(Clone, Copy, Default)]
struct HoverOverlayState {
    host: bool,
    overlay: bool,
    menu: bool,
}

impl HoverOverlayState {
    fn revealed(self) -> bool {
        self.host || self.overlay || self.menu
    }
}

/// Callback invoked whenever a [`HoverOverlay`]'s unified revealed state transitions.
type RevealCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Builder closure for a [`HoverOverlay`] element tree.
type HoverOverlayBuilder = Box<dyn FnOnce(HoverHandle, &mut Window, &mut App) -> AnyElement>;

/// Handle passed to [`HoverOverlay`]'s builder so the host element, its overhanging floating
/// overlay, and any dropdown menus inside the overlay share a single hover lifecycle.
#[derive(Clone)]
pub struct HoverHandle {
    state: Entity<HoverOverlayState>,
    on_revealed: Option<RevealCallback>,
}

impl HoverHandle {
    /// Whether the host, its overhanging overlay, or an open menu inside the overlay is active.
    pub fn revealed(&self, cx: &App) -> bool {
        self.state.read(cx).revealed()
    }

    /// Attach to the host element (`Stateful<Div>::on_hover`).
    pub fn host_on_hover(&self) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        self.transition(|s, v| s.host = v)
    }

    /// Attach to the overhanging interactive overlay (`Stateful<Div>::on_hover`).
    pub fn overlay_on_hover(&self) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        self.transition(|s, v| s.overlay = v)
    }

    /// Attach to a dropdown menu (`DropdownMenuPopover::on_open_change`) inside the overlay so
    /// moving the pointer into the popup menu keeps the overlay mounted until dismissal.
    pub fn menu_on_open_change(&self) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        self.transition(|s, v| s.menu = v)
    }

    fn transition(
        &self,
        set: fn(&mut HoverOverlayState, bool),
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let state = self.state.clone();
        let on_revealed = self.on_revealed.clone();
        move |next: &bool, window: &mut Window, cx: &mut App| {
            let (was, now) = state.update(cx, |s, _| {
                let was = s.revealed();
                set(s, *next);
                (was, s.revealed())
            });
            if was != now {
                if let Some(cb) = &on_revealed {
                    cb(now, window, cx);
                }
                window.refresh();
            }
        }
    }
}

/// Wraps a host element and its overhanging floating interaction panel so moving the pointer
/// off the host onto the overhanging portion of the overlay keeps the panel open, keeps the
/// host highlighted, and blocks non-scroll mouse events (`.block_mouse_except_scroll()`) on
/// whatever sits beneath the overlay; while hidden, the overlay is omitted completely so it
/// never steals hover or clicks from neighboring rows.
#[derive(IntoElement)]
pub struct HoverOverlay {
    id: ElementId,
    on_revealed: Option<RevealCallback>,
    render: HoverOverlayBuilder,
}

impl HoverOverlay {
    pub fn new(
        id: impl Into<ElementId>,
        render: impl FnOnce(HoverHandle, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            on_revealed: None,
            render: Box::new(render),
        }
    }

    pub fn on_revealed(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_revealed = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for HoverOverlay {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("hover-overlay:{}", self.id)),
            cx,
            |_, _| HoverOverlayState::default(),
        );
        let handle = HoverHandle {
            state,
            on_revealed: self.on_revealed,
        };
        (self.render)(handle, window, cx)
    }
}

/// The floating hover bar of one message row; mounted while `hover.revealed(cx)` is true and
/// kept open while the pointer is anywhere inside the bar (including the overhanging top half
/// above the row's bounds) or inside its "More actions" menu.
pub fn hover_toolbar(
    key: &str,
    quick: &[String],
    set: ActionSet,
    handler: &ActionHandler,
    hover: &HoverHandle,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    // Like Slack's, the bar straddles the row's top edge. Blocking non-scroll mouse events
    // prevents the row or divider underneath the top half from stealing hover/clicks while
    // keeping wheel/trackpad scrolling over the toolbar smooth and uninterrupted.
    let mut bar = h_flex()
        .id(SharedString::from(format!("{key}-toolbar")))
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .shadow_sm()
        .absolute()
        .top(px(-16.))
        .right_2()
        .block_mouse_except_scroll()
        .on_hover(hover.overlay_on_hover());
    for name in quick {
        let Some(glyph) = emoji::glyph(name) else {
            continue;
        };
        bar = bar.child(
            toolbar_button(
                format!("{key}-quick-{name}"),
                handler,
                MessageAction::React(name.clone()),
            )
            .label(glyph)
            .tooltip(format!(":{name}:")),
        );
    }
    let pick = handler.clone();
    bar = bar.child(
        Button::new(SharedString::from(format!("{key}-react")))
            .ghost()
            .with_size(BAR_BUTTON)
            .icon(IconName::FaceSlightlySmilingPlus)
            .tooltip("Add reaction")
            .on_click(move |event, window, cx| {
                pick(MessageAction::PickReaction(event.position()), window, cx);
            }),
    );
    if set.reply {
        bar = bar.child(
            toolbar_button(
                format!("{key}-reply"),
                handler,
                MessageAction::ReplyInThread,
            )
            .icon(IconName::MessageSquareReply)
            .tooltip("Reply in thread"),
        );
    }
    let menu_handler = handler.clone();
    bar.child(
        toolbar_button(format!("{key}-forward"), handler, MessageAction::Forward)
            .icon(IconName::Forward)
            .tooltip("Forward message"),
    )
    .child(
        toolbar_button(format!("{key}-save"), handler, MessageAction::SaveForLater)
            .icon(IconName::Bookmark)
            .tooltip(if set.saved {
                "Remove from saved"
            } else {
                "Save for later"
            }),
    )
    .child(
        Button::new(SharedString::from(format!("{key}-more")))
            .ghost()
            .with_size(BAR_BUTTON)
            .icon(IconName::EllipsisVertical)
            .tooltip("More actions")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                actions_menu(menu, set, &menu_handler, MenuOrigin::More, window, cx)
            })
            .on_open_change(hover.menu_on_open_change()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::v_flex;
    use gpui_kit::gpui::prelude::FluentBuilder as _;
    use gpui_kit::gpui::{
        self, Bounds, Context, Modifiers, MouseButton, Pixels, Render, canvas, div, point,
    };
    use std::cell::{Cell, RefCell};

    #[gpui::test]
    fn hover_toolbar_stays_open_and_clickable_in_overhanging_top_half(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });

        struct Harness {
            above_clicks: Rc<Cell<u32>>,
            picked: Rc<Cell<u32>>,
            scroll_events: Rc<Cell<u32>>,
            revealed_now: Rc<Cell<bool>>,
            row_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
            bar_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
        }

        impl Render for Harness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let above_clicks = self.above_clicks.clone();
                let picked = self.picked.clone();
                let scroll_events = self.scroll_events.clone();
                let revealed_now = self.revealed_now.clone();
                let row_bounds = self.row_bounds.clone();
                let bar_bounds = self.bar_bounds.clone();
                let handler: ActionHandler = Rc::new(move |action, _, _| {
                    if matches!(action, MessageAction::PickReaction(_)) {
                        picked.set(picked.get() + 1);
                    }
                });
                let set = ActionSet {
                    own: false,
                    reply: true,
                    saved: false,
                    pinned: false,
                    in_thread: false,
                };
                v_flex()
                    .id("scroll-host")
                    .w(px(500.))
                    .h(px(200.))
                    .overflow_y_scroll()
                    .on_scroll(move |_, _, _| {
                        scroll_events.set(scroll_events.get() + 1);
                    })
                    .child(
                        // Upper row / divider right above the target message.
                        div()
                            .id("upper-row")
                            .w_full()
                            .h(px(60.))
                            .on_click(move |_, _, _| above_clicks.set(above_clicks.get() + 1)),
                    )
                    .child(HoverOverlay::new("msg-b", move |hover, _, cx| {
                        let revealed = hover.revealed(cx);
                        revealed_now.set(revealed);
                        let rb = row_bounds.clone();
                        let bb = bar_bounds.clone();
                        div()
                            .id("msg-b")
                            .relative()
                            .w_full()
                            .h(px(60.))
                            .child(
                                canvas(
                                    move |bounds, _, _| *rb.borrow_mut() = Some(bounds),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .when(revealed, |row| {
                                row.child(
                                    hover_toolbar("msg-b", &[], set, &handler, &hover, cx).child(
                                        canvas(
                                            move |bounds, _, _| {
                                                *bb.borrow_mut() = Some(bounds);
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .inset_0(),
                                    ),
                                )
                            })
                            .on_hover(hover.host_on_hover())
                            .into_any_element()
                    }))
            }
        }

        let above_clicks = Rc::new(Cell::new(0));
        let picked = Rc::new(Cell::new(0));
        let scroll_events = Rc::new(Cell::new(0));
        let revealed_now = Rc::new(Cell::new(false));
        let row_bounds = Rc::new(RefCell::new(None));
        let bar_bounds = Rc::new(RefCell::new(None));

        let (_view, cx) = cx.add_window_view({
            let (above_clicks, picked, scroll_events, revealed_now, row_bounds, bar_bounds) = (
                above_clicks.clone(),
                picked.clone(),
                scroll_events.clone(),
                revealed_now.clone(),
                row_bounds.clone(),
                bar_bounds.clone(),
            );
            move |_, _| Harness {
                above_clicks,
                picked,
                scroll_events,
                revealed_now,
                row_bounds,
                bar_bounds,
            }
        });
        cx.run_until_parked();

        // 1. Before hovering msg-b, clicking in the upper row (where msg-b's top half will hang)
        //    hits the upper row and does NOT reveal msg-b's toolbar.
        let rb = row_bounds.borrow().expect("row bounds recorded");
        let overhang_y = rb.origin.y - px(6.);
        let probe_x = rb.right() - px(160.);
        cx.simulate_mouse_move(point(probe_x, overhang_y), None, Modifiers::none());
        cx.run_until_parked();
        assert!(
            !revealed_now.get(),
            "hovering above msg-b must not reveal hidden toolbar"
        );
        cx.simulate_click(point(probe_x, overhang_y), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(above_clicks.get(), 1);
        assert_eq!(picked.get(), 0);

        // 2. Hover inside msg-b -> toolbar appears and overhangs above msg-b.
        cx.simulate_mouse_move(
            point(rb.origin.x + px(40.), rb.origin.y + px(30.)),
            None,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert!(revealed_now.get(), "hovering msg-b must reveal toolbar");
        let bb = bar_bounds.borrow().expect("toolbar bounds recorded");
        assert!(
            bb.origin.y < rb.origin.y,
            "toolbar top ({:?}) must overhang above row top ({:?})",
            bb.origin.y,
            rb.origin.y
        );

        // 3. Move pointer into the TOP HALF of the first button (Add reaction smile icon),
        //    strictly ABOVE msg-b's top edge -> toolbar must STAY open and clicking it must
        //    trigger PickReaction WITHOUT clicking the upper row underneath.
        let smile_top_half = point(bb.origin.x + px(16.), bb.origin.y + px(6.));
        assert!(smile_top_half.y < rb.origin.y);
        cx.simulate_mouse_move(smile_top_half, None, Modifiers::none());
        cx.run_until_parked();
        assert!(
            revealed_now.get(),
            "hovering the overhanging top half of the toolbar must keep it open"
        );
        cx.simulate_mouse_down(smile_top_half, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(smile_top_half, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            picked.get(),
            1,
            "clicking top half of smile button must trigger PickReaction"
        );
        assert_eq!(
            above_clicks.get(),
            1,
            "occluded upper row must not receive clicks through toolbar"
        );

        // 3b. Scrolling while the pointer intersects the hover toolbar must still reach the
        //     underlying scroll container instead of aborting scroll.
        cx.simulate_event(gpui::ScrollEvent {
            position: smile_top_half,
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-40.))),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(
            scroll_events.get(),
            1,
            "scroll wheel over hover toolbar must pass through to the scrollable list"
        );

        // 4. Move pointer completely outside both msg-b and its toolbar -> toolbar closes.
        cx.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::none());
        cx.run_until_parked();
        assert!(
            !revealed_now.get(),
            "leaving both row and toolbar must close toolbar"
        );
    }
}
