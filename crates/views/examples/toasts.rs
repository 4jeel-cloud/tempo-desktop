use std::time::Duration;

use gpui::prelude::*;
use gpui::{App, Context, Render, SharedString, Task, Window, div};
use i18n::t;
use state::{Outcome, Toast, Toasts};
use ui::{ActiveTheme as _, Button, Text, eyebrow, heading};

/// How long a sample promise works before it settles.
const WORK: Duration = Duration::from_secs(2);
/// How many toasts the burst sample shows.
const BURST: usize = 5;
/// The name the samples put in bold.
const NAME: &str = "Midnight City";

/// One button on the page: its id, the Fluent key of its label, and the toast it shows.
type Sample = (
    &'static str,
    &'static str,
    fn(&mut ToastsView, &mut Context<ToastsView>),
);

/// A page with a button for every kind of toast, so each one can be tried without waiting for
/// the app to need it.
pub(crate) struct ToastsView {
    work: Vec<Task<()>>,
}

impl ToastsView {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self { work: Vec::new() }
    }

    /// Shows a loading toast and settles it into a success or a failure once the work is done.
    fn promise(&mut self, succeeds: bool, cx: &mut Context<Self>) {
        let id = Toasts::post(
            Toast::new(Outcome::Loading, "toast-sample-saving").name(NAME),
            cx,
        );
        self.work.push(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(WORK).await;
            cx.update(|cx| {
                let settled = match succeeds {
                    true => Toast::new(Outcome::Done, "toast-sample-saved").name(NAME),
                    false => Toast::new(Outcome::Failed, "toast-sample-save-failed")
                        .name(NAME)
                        .detail(t!("toast-sample-save-failed-detail")),
                };
                Toasts::settle(id, settled, cx);
            });
        }));
    }

    fn group(
        &self,
        key: &'static str,
        samples: &[Sample],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(eyebrow(i18n::lookup(key, None), cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(samples.iter().map(|(id, label, show)| {
                        let show = *show;
                        Button::new(*id)
                            .outline()
                            .label(i18n::lookup(label, None))
                            .on_click(cx.listener(move |this, _, _, cx| show(this, cx)))
                    })),
            )
    }
}

impl Render for ToastsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();

        let kinds: [Sample; 6] = [
            ("toast-plain", "toasts-show-plain", |_, cx| {
                Toasts::show(Outcome::Plain, "toast-sample-plain", cx)
            }),
            ("toast-done", "toasts-show-done", |_, cx| {
                Toasts::show(Outcome::Done, "toast-sample-done", cx)
            }),
            ("toast-info", "toasts-show-info", |_, cx| {
                Toasts::show(Outcome::Info, "toast-sample-info", cx)
            }),
            ("toast-warning", "toasts-show-warning", |_, cx| {
                Toasts::show(Outcome::Warning, "toast-sample-warning", cx)
            }),
            ("toast-failed", "toasts-show-failed", |_, cx| {
                Toasts::show(Outcome::Failed, "toast-sample-failed", cx)
            }),
            ("toast-loading", "toasts-show-loading", |_, cx| {
                Toasts::post(Toast::new(Outcome::Loading, "toast-sample-loading"), cx);
            }),
        ];
        let content: [Sample; 5] = [
            ("toast-detail", "toasts-show-detail", |_, cx| {
                Toasts::with_detail(
                    Outcome::Plain,
                    "toast-sample-event",
                    None,
                    t!("toast-sample-event-detail"),
                    cx,
                )
            }),
            ("toast-action", "toasts-show-action", |_, cx| {
                let toast = Toast::new(Outcome::Plain, "toast-sample-event")
                    .detail(t!("toast-sample-event-detail"))
                    .action("toast-sample-undo", undo);
                Toasts::post(toast, cx);
            }),
            ("toast-cancel", "toasts-show-cancel", |_, cx| {
                let toast = Toast::new(Outcome::Plain, "toast-sample-delete")
                    .name(NAME)
                    .cancel("toast-sample-cancel", |_| {})
                    .action("toast-sample-undo", undo);
                Toasts::post(toast, cx);
            }),
            ("toast-name", "toasts-show-name", |_, cx| {
                Toasts::about(Outcome::Done, "toast-sample-queued", NAME, cx)
            }),
            ("toast-long", "toasts-show-long", |_, cx| {
                Toasts::show(Outcome::Info, "toast-sample-long", cx)
            }),
        ];
        let behaviour: [Sample; 5] = [
            ("toast-promise", "toasts-show-promise", |this, cx| {
                this.promise(true, cx)
            }),
            (
                "toast-promise-failed",
                "toasts-show-promise-failed",
                |this, cx| this.promise(false, cx),
            ),
            ("toast-lasting", "toasts-show-lasting", |_, cx| {
                Toasts::post(
                    Toast::new(Outcome::Info, "toast-sample-lasting").lasting(),
                    cx,
                );
            }),
            ("toast-burst", "toasts-show-burst", |_, cx| {
                for number in 1..=BURST {
                    Toasts::about(
                        Outcome::Plain,
                        "toast-sample-burst",
                        SharedString::from(number.to_string()),
                        cx,
                    );
                }
            }),
            ("toast-clear", "toasts-clear", |_, cx| Toasts::clear(cx)),
        ];

        div()
            .id("toasts-page")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .w_full()
                    .p_6()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(heading(t!("toasts-page-title"), cx))
                            .child(
                                div()
                                    .text_size(theme.text(Text::Label))
                                    .text_color(theme.muted_foreground)
                                    .child(t!("toasts-page-detail")),
                            ),
                    )
                    .child(self.group("toasts-group-kinds", &kinds, cx))
                    .child(self.group("toasts-group-content", &content, cx))
                    .child(self.group("toasts-group-behaviour", &behaviour, cx)),
            )
    }
}

/// The sample action, which only says it ran.
fn undo(cx: &mut App) {
    Toasts::show(Outcome::Info, "toast-sample-undone", cx);
}
