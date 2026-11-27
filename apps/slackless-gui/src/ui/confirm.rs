//! The one confirm dialog: a title, a body, Cancel and a single action button, built on
//! gpui-component's `AlertDialog` and `DialogButtonProps` so the kit owns layout, focus,
//! dismissal and the entrance animation.

use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{AnyElement, App, ParentElement as _, Pixels, SharedString, Window, px};

/// Content a confirm shows under its body: the URL about to open, the secret lines found, ...
type Extra = Rc<dyn Fn(&App) -> AnyElement>;

/// What runs when a confirm is dismissed without confirming.
type OnCancel = Rc<dyn Fn(&mut Window, &mut App)>;

/// A confirmation prompt. [`Self::open`] shows Cancel and one action button; Cancel, Escape
/// and the close button dismiss it, Enter and the action button run the confirmation.
pub struct ConfirmDialog {
    title: SharedString,
    body: SharedString,
    action: SharedString,
    variant: ButtonVariant,
    width: Pixels,
    extra: Option<Extra>,
    on_cancel: Option<OnCancel>,
}

impl ConfirmDialog {
    /// A confirm whose action is the primary button; [`Self::danger`] turns it red.
    pub fn new(
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        action: impl Into<SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            action: action.into(),
            variant: ButtonVariant::Primary,
            width: px(420.),
            extra: None,
            on_cancel: None,
        }
    }

    /// The action removes or destroys something.
    pub fn danger(mut self) -> Self {
        self.variant = ButtonVariant::Danger;
        self
    }

    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.width = width.into();
        self
    }

    pub fn extra(mut self, extra: impl Fn(&App) -> AnyElement + 'static) -> Self {
        self.extra = Some(Rc::new(extra));
        self
    }

    pub fn on_cancel(mut self, on_cancel: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(on_cancel));
        self
    }

    /// Open the dialog over `window`; `on_confirm` runs when the action button is clicked.
    pub fn open(
        self,
        window: &mut Window,
        cx: &mut App,
        on_confirm: impl Fn(&mut Window, &mut App) + 'static,
    ) {
        let on_confirm = Rc::new(on_confirm);
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let (run, cancel) = (on_confirm.clone(), self.on_cancel.clone());
            alert
                .title(self.title.clone())
                .description(self.body.clone())
                .width(self.width)
                .close_button(true)
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(self.action.clone())
                        .ok_variant(self.variant)
                        .on_ok(move |_, window, cx| {
                            run(window, cx);
                            true
                        })
                        .on_cancel(move |_, window, cx| {
                            if let Some(cancel) = &cancel {
                                cancel(window, cx);
                            }
                            true
                        }),
                )
                .when_some(self.extra.clone(), |alert, extra| alert.child(extra(cx)))
        });
    }
}
