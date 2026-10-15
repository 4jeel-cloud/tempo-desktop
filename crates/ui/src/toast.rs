use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Interactivity, Pixels, SharedString,
    Stateful, StyleRefinement, Window, div, px, svg,
};

use crate::button::Button;
use crate::metrics::Text;
use crate::modal::corners;
use crate::spinner::Spinner;
use crate::theme::ActiveTheme as _;

const ICON: Pixels = px(16.);
/// The card's border width, which the height it reports has to count on both edges.
const BORDER: Pixels = px(1.);

type Press = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type Measure = Box<dyn Fn(Pixels, &mut Window, &mut App) + 'static>;

/// One toast card: an optional icon, a message with an optional muted line under it, up to two
/// buttons and a close button. The card fills the width it is given, and a caller that sets its
/// height clips the contents, which is how the toast stack piles cards behind the front one. The
/// card does not block the pointer itself, so the container it sits in stays hovered over it and
/// has to occlude what is behind.
#[derive(IntoElement)]
pub struct Toast {
    base: Stateful<Div>,
    id: ElementId,
    message: SharedString,
    strong: Option<SharedString>,
    detail: Option<SharedString>,
    icon: Option<SharedString>,
    loading: bool,
    action: Option<(SharedString, Press)>,
    cancel: Option<(SharedString, Press)>,
    shown: f32,
    dismiss: Option<Press>,
    open: Option<Press>,
    measure: Option<Measure>,
    inert: bool,
}

impl Toast {
    #[track_caller]
    pub fn new(id: impl Into<ElementId>, message: impl Into<SharedString>) -> Self {
        let id = id.into();

        Self {
            base: div().id(id.clone()),
            id,
            message: message.into(),
            strong: None,
            detail: None,
            icon: None,
            loading: false,
            action: None,
            cancel: None,
            shown: 1.,
            dismiss: None,
            open: None,
            measure: None,
            inert: false,
        }
    }

    pub fn strong(mut self, name: impl Into<SharedString>) -> Self {
        self.strong = Some(name.into());
        self
    }

    /// A muted line below the message, wrapping rather than truncating, such as why it failed.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.icon = Some(path.into());
        self
    }

    /// Shows a turning spinner in place of the icon.
    pub fn loading(mut self) -> Self {
        self.loading = true;
        self
    }

    /// The outlined button beside the message, such as Undo.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((label.into(), Box::new(handler)));
        self
    }

    /// The quiet button before the action, such as Cancel.
    pub fn cancel(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.cancel = Some((label.into(), Box::new(handler)));
        self
    }

    /// Keeps the buttons drawn but unpressable, for a card on its way out, so nothing reflows and
    /// a press reaches the card under it.
    pub fn inert(mut self) -> Self {
        self.inert = true;
        self
    }

    /// How much of the contents show, from 0 to 1, while the card itself stays. A card stacked
    /// behind another hides its contents.
    pub fn contents(mut self, shown: f32) -> Self {
        self.shown = shown.clamp(0., 1.);
        self
    }

    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.dismiss = Some(Box::new(handler));
        self
    }

    pub fn on_open(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.open = Some(Box::new(handler));
        self
    }

    /// Reports the height the card needs to show all of its contents, borders included, on every
    /// frame and whatever height the card is held to. The stack uses it to size the card and the
    /// ones piled behind it.
    pub fn on_measure(mut self, handler: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.measure = Some(Box::new(handler));
        self
    }
}

impl Styled for Toast {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for Toast {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Toast {}

impl RenderOnce for Toast {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            mut base,
            id,
            message,
            strong,
            detail,
            icon,
            loading,
            action,
            cancel,
            shown,
            dismiss,
            open,
            measure,
            inert,
        } = self;

        let theme = *cx.theme();
        let pad = theme.metrics.pad * 2;
        let overrides = std::mem::take(base.style());

        let leading = match (loading, icon) {
            (true, _) => Some(
                Spinner::new((id.clone(), "spinner"))
                    .size(ICON)
                    .text_color(theme.popover_foreground)
                    .into_any_element(),
            ),
            (false, Some(icon)) => Some(
                svg()
                    .path(icons::path(icon))
                    .size(ICON)
                    .flex_none()
                    .text_color(theme.popover_foreground)
                    .into_any_element(),
            ),
            (false, None) => None,
        };

        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .child(
                with_name_emphasised(&id, message, strong, open)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.popover_foreground),
            )
            .when_some(detail, |text, detail| {
                text.child(div().text_color(theme.muted_foreground).child(detail))
            });

        let body = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .w_full()
            .p(pad)
            .opacity(shown)
            .children(leading)
            .child(text)
            .when(cancel.is_some() || action.is_some(), |body| {
                body.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1p5()
                        .children(cancel.map(|(label, press)| {
                            Button::new((id.clone(), "cancel"))
                                .ghost()
                                .small()
                                .label(label)
                                .when(!inert, |button| {
                                    button
                                        .on_click(move |event, window, cx| press(event, window, cx))
                                })
                        }))
                        .children(action.map(|(label, press)| {
                            Button::new((id.clone(), "action"))
                                .outline()
                                .small()
                                .label(label)
                                .when(!inert, |button| {
                                    button
                                        .on_click(move |event, window, cx| press(event, window, cx))
                                })
                        })),
                )
            })
            .children(dismiss.map(|dismiss| {
                Button::new((id.clone(), "dismiss"))
                    .ghost()
                    .small()
                    .icon("icons/x.svg")
                    .when(!inert, |button| {
                        button
                            .tooltip("common-dismiss")
                            .on_click(move |event, window, cx| dismiss(event, window, cx))
                    })
            }));

        let contents = match measure {
            Some(measure) => div()
                .w_full()
                .on_children_prepainted(move |bounds, window, cx| {
                    if let Some(body) = bounds.first() {
                        measure(body.size.height + BORDER * 2., window, cx);
                    }
                })
                .child(body),
            None => div().w_full().child(body),
        };

        let mut toast = base
            .w_full()
            .overflow_hidden()
            .rounded(corners(theme.radius))
            .border(BORDER)
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .text_size(theme.text(Text::Label))
            .text_color(theme.popover_foreground)
            .child(contents);
        toast.style().refine(&overrides);
        toast
    }
}

fn with_name_emphasised(
    id: &ElementId,
    message: SharedString,
    strong: Option<SharedString>,
    open: Option<Press>,
) -> Div {
    let split = strong
        .as_ref()
        .and_then(|name| message.find(name.as_ref()).map(|at| (at, name)));

    let Some((at, name)) = split else {
        return div().min_w_0().child(message);
    };

    div()
        .flex()
        .min_w_0()
        .child(
            div()
                .flex_none()
                .child(SharedString::from(message[..at].to_owned())),
        )
        .child(name_span(id, name.clone(), open))
        .child(
            div()
                .flex_none()
                .child(SharedString::from(message[at + name.len()..].to_owned())),
        )
}

fn name_span(id: &ElementId, name: SharedString, open: Option<Press>) -> AnyElement {
    let span = div().min_w_0().truncate().font_weight(FontWeight::BOLD);

    let Some(open) = open else {
        return span.child(name).into_any_element();
    };

    span.id((id.clone(), "link"))
        .cursor_pointer()
        .hover(|style| style.underline())
        .on_click(move |event, window, cx| open(event, window, cx))
        .child(name)
        .into_any_element()
}
