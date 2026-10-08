//! Port of src/renderer/src/components/Draft.tsx.
//!
//! A remark written but not yet sent.
//!
//! Dashed rather than solid, and badged: it has to be obvious at a glance which
//! remarks are still yours to change and which the author has already been told
//! about, because they sit side by side on the same line.
//!
//! The React component took `onEdit` / `onDelete` from its owner. Here the card calls
//! [`AppState::update_draft`] and [`AppState::remove_draft`] itself. Both are
//! synchronous, so the TSX's busy state (which only guarded two overlapping awaits)
//! has nothing to guard. The state notifies on every draft change, and whoever lists
//! the drafts observes it.

use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Window, div, prelude::*,
};
use reviewdeck_core::model::DraftComment;

use crate::state::{AppState, GlobalState};
use crate::ui::app_view::toast;
use crate::ui::components::badge::{Badge, BadgeTone};
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant, with_alpha};
use crate::ui::components::input::{TextInput, TextInputEvent};
use crate::ui::components::toast::ToastKind;
use crate::ui::icons::IconName;
use crate::ui::markdown_view::MarkdownView;
use crate::ui::theme::{ActiveTheme, UI_FONT, radius, rpx};
use crate::ui::thread_view::{image_loader, markdown_context};

/// The editor that replaces the body while the draft is being changed.
struct Editor {
    input: Entity<TextInput>,
    _subscription: Subscription,
}

/// One pending line comment.
pub struct DraftCard {
    draft: DraftComment,
    state: Entity<AppState>,
    /// The rendered body, kept so showing the card again does not parse it again.
    body: Entity<MarkdownView>,
    editor: Option<Editor>,
    _subscriptions: Vec<Subscription>,
}

impl DraftCard {
    pub fn new(draft: DraftComment, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        let context = markdown_context(&draft.item_id, cx);
        let images = image_loader(state.clone());
        let source = SharedString::from(draft.body.clone());
        let body = cx.new(|cx| MarkdownView::new(source, context, Some(images), true, cx));
        let subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        DraftCard {
            draft,
            state,
            body,
            editor: None,
            _subscriptions: subscriptions,
        }
    }

    /// Replaces the draft (after the owner re-listed them). An edit in progress stays.
    pub fn set_draft(&mut self, draft: DraftComment, cx: &mut Context<Self>) {
        if self.draft == draft {
            return;
        }
        let context = markdown_context(&draft.item_id, cx);
        let source = SharedString::from(draft.body.clone());
        self.body
            .update(cx, |body, cx| body.set_source(source, context, cx));
        self.draft = draft;
        cx.notify();
    }

    /// The edit button: the field starts from the stored body, whatever an earlier,
    /// abandoned edit left behind, and takes the caret.
    pub fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            return;
        }
        let text = self.draft.body.clone();
        let input = cx.new(|cx| TextInput::new(cx).multi_line(3, 10).with_text(text));
        let subscription = cx.subscribe(
            &input,
            |this: &mut Self, _input, event: &TextInputEvent, cx| match event {
                TextInputEvent::Changed => cx.notify(),
                TextInputEvent::Submit => this.save(cx),
                TextInputEvent::Cancel => this.cancel_edit(cx),
            },
        );
        input.read(cx).focus(window);
        self.editor = Some(Editor {
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        cx.notify();
    }

    /// Save, also on ⌘↵: an empty body is not saved.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let body = editor.input.read(cx).text().trim().to_string();
        if body.is_empty() {
            return;
        }
        let id = self.draft.id.clone();
        let result = self
            .state
            .update(cx, |state, cx| state.update_draft(&id, &body, cx));
        match result {
            Ok(_) => self.editor = None,
            // The editor stays open with what was typed, as it did behind the TSX toast.
            Err(error) => toast(cx, ToastKind::Bad, error.to_string()),
        }
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let id = self.draft.id.clone();
        self.state.update(cx, |state, cx| {
            state.remove_draft(&id, cx);
        });
    }

    fn actions(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .ml_auto()
            .flex()
            .items_center()
            .gap(rpx(2.))
            .child(
                Button::new("draft-edit")
                    .variant(ButtonVariant::Ghost)
                    .icon_only(IconName::Pencil)
                    .tooltip("Edit this draft")
                    .on_click(cx.listener(|this, _, window, cx| this.start_edit(window, cx))),
            )
            .child(
                Button::new("draft-delete")
                    .variant(ButtonVariant::Ghost)
                    .icon_only(IconName::Trash2)
                    .tooltip("Delete this draft")
                    .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
            )
    }

    fn editor_view(&self, editor: &Editor, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;
        let empty = editor.input.read(cx).text().trim().is_empty();
        div().child(editor.input.clone()).child(
            div()
                .mt(rpx(6.))
                .flex()
                .items_center()
                .justify_end()
                .gap(rpx(6.))
                .child(
                    div()
                        .mr_auto()
                        .text_size(rpx(10.5))
                        .text_color(colors.muted_foreground)
                        .child("⌘↵ to save"),
                )
                .child(
                    Button::new("draft-cancel")
                        .size(ButtonSize::Sm)
                        .variant(ButtonVariant::Ghost)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_edit(cx)))
                        .child("Cancel"),
                )
                .child(
                    Button::new("draft-save")
                        .size(ButtonSize::Sm)
                        .variant(ButtonVariant::Default)
                        .disabled(empty)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                        .child("Save"),
                ),
        )
    }
}

impl Render for DraftCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let editing = self.editor.is_some();
        let draft = &self.draft;
        // The last line of the range is the one the comment is anchored to.
        let range = draft.range.as_ref().map(|range| {
            let end = draft.new_line.or(draft.old_line);
            match end {
                Some(end) => format!("Lines {}-{}", range.start_line, end),
                None => format!("Lines {}-", range.start_line),
            }
        });

        div()
            .id("draft")
            .font_family(UI_FONT)
            .m(rpx(6.))
            .rounded(rpx(radius::MD))
            .border_1()
            .border_dashed()
            .border_color(with_alpha(colors.info, 0.5))
            .bg(colors.info_soft)
            .px(rpx(10.))
            .py(rpx(8.))
            .child(
                div()
                    .mb(rpx(4.))
                    .flex()
                    .items_center()
                    .gap(rpx(6.))
                    .child(Badge::new().tone(BadgeTone::Info).child("Pending"))
                    .children(range.map(|range| {
                        div()
                            .text_size(rpx(11.))
                            .text_color(colors.muted_foreground)
                            .child(range)
                    }))
                    .when(!editing, |row| row.child(self.actions(cx))),
            )
            .child(match &self.editor {
                Some(editor) => self.editor_view(editor, cx).into_any_element(),
                None => self.body.clone().into_any_element(),
            })
    }
}
