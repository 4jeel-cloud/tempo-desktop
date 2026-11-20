use gpui::prelude::*;
use gpui::{Context, Entity, Pixels, Render, WeakEntity, Window, div, px};
use input::SEARCH_CONTEXT;
use router::{Destination, navigate};
use state::{SessionEvent, Sonora};
use ui::{Input, SelectNext, SelectPrevious};

use crate::screens::search::SearchView;

const WIDEST: Pixels = px(640.);

/// The search field that lives in the title bar, so search is a keystroke away from every
/// screen. It owns the one input the search results are read from.
pub(crate) struct SearchBar {
    input: Entity<Input>,
    search: WeakEntity<SearchView>,
}

impl SearchBar {
    pub fn new(input: Entity<Input>, search: &Entity<SearchView>, cx: &mut Context<Self>) -> Self {
        cx.observe(&input, |_, input, cx| {
            if input.read(cx).text().trim().is_empty() {
                return;
            }
            if !matches!(router::trail(cx).read(cx).current(), Destination::Search) {
                navigate(Destination::Search, cx);
            }
        })
        .detach();

        let session = Sonora::global(cx).session.clone();
        cx.subscribe(&session, |this, _, event, cx| {
            if matches!(event, SessionEvent::SignedOut) {
                this.input.update(cx, |input, cx| input.set_text("", cx));
            }
        })
        .detach();

        Self {
            input,
            search: search.downgrade(),
        }
    }

    /// The results view when it is showing. A key pressed on another screen opens search instead.
    fn results(&self, cx: &mut Context<Self>) -> Option<Entity<SearchView>> {
        let search = self.search.upgrade()?;
        if matches!(router::trail(cx).read(cx).current(), Destination::Search) {
            return Some(search);
        }
        navigate(Destination::Search, cx);
        None
    }
}

impl Render for SearchBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .max_w(WIDEST)
            .key_context(SEARCH_CONTEXT)
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                let Some(search) = this.results(cx) else {
                    return;
                };
                search.update(cx, |view, cx| view.select_next(&SelectNext, window, cx));
            }))
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                let Some(search) = this.results(cx) else {
                    return;
                };
                search.update(cx, |view, cx| {
                    view.select_previous(&SelectPrevious, window, cx)
                });
            }))
            .child(self.input.clone())
    }
}
