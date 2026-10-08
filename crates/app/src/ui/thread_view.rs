//! Port of src/renderer/src/components/Thread.tsx.
//!
//! One conversation, wherever it appears.
//!
//! What is on offer comes from the thread's own capability flags, never from which
//! host it came from - so a reply box only exists where a reply can actually be
//! sent, and the resolve control only where the host can resolve.
//!
//! The React component took `onReply` / `onResolve` callbacks from its owner. Here the
//! card performs both itself through [`AppState`] (`reply_to_thread`,
//! `set_thread_resolved`), keeps its own busy and error state, and tells its owner with
//! [`ThreadEvent::Changed`] once the thread on the host is different, so the owner
//! reloads the threads. In the TSX "the caller has already surfaced" a failure; with no
//! caller to do it, the card shows the message itself, under the controls it came from.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Window, div, prelude::*,
};
use reviewdeck_core::autolink::repository_root_of;
use reviewdeck_core::images::ImageAccount;
use reviewdeck_core::markdown::{AutolinkContext, MarkdownContext};
use reviewdeck_core::model::{CommentThread, PullComment};
use reviewdeck_core::time::{now_ms, relative_time};

use crate::state::{AppState, GlobalState};
use crate::ui::components::avatar::Avatar;
use crate::ui::components::badge::{Badge, BadgeTone};
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant};
use crate::ui::components::glass::GlassExt;
use crate::ui::components::input::{TextInput, TextInputEvent};
use crate::ui::icons::{Icon, IconName};
use crate::ui::markdown_view::{ImageLoader, MarkdownView};
use crate::ui::theme::{ActiveTheme, MONO_FONT, UI_FONT, radius, rpx};

/// What a card tells its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadEvent {
    /// A reply was posted or the resolved flag was toggled: the thread on the host is
    /// not the one the card holds any more, so the owner reloads the threads.
    Changed,
}

/// The reply composer, which exists only while the user is replying.
struct ReplyBox {
    input: Entity<TextInput>,
    /// A reply is on its way: the field and both buttons are disabled.
    busy: bool,
    /// Why the last attempt failed. What was typed is kept.
    error: Option<SharedString>,
    _subscription: Subscription,
}

/// One thread: header, comments, controls and the reply composer.
pub struct ThreadCard {
    item_id: String,
    thread: CommentThread,
    /// Tighter typography and spacing, for a thread sitting inside a diff row.
    dense: bool,
    state: Entity<AppState>,
    /// One markdown entity per comment id, so rendering the card again, or receiving the
    /// same thread after a reload, never parses a body twice.
    bodies: HashMap<String, Entity<MarkdownView>>,
    reply: Option<ReplyBox>,
    /// Resolve or reopen is on its way.
    resolving: bool,
    resolve_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadEvent> for ThreadCard {}

impl ThreadCard {
    pub fn new(
        item_id: String,
        thread: CommentThread,
        dense: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        // An authenticated image that has landed is announced by the state; the bodies
        // draw it on their next frame, which this brings about.
        let subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        let mut card = ThreadCard {
            item_id,
            thread,
            dense,
            state,
            bodies: HashMap::new(),
            reply: None,
            resolving: false,
            resolve_error: None,
            _subscriptions: subscriptions,
        };
        card.sync_bodies(cx);
        card
    }

    /// Replaces the thread (after a reload). The reply draft, the open composer and the
    /// busy state stay as they are.
    pub fn set_thread(&mut self, thread: CommentThread, cx: &mut Context<Self>) {
        if self.thread == thread {
            return;
        }
        self.thread = thread;
        self.sync_bodies(cx);
        cx.notify();
    }

    /// Makes `bodies` hold exactly one up-to-date view per comment of the thread.
    fn sync_bodies(&mut self, cx: &mut Context<Self>) {
        let context = markdown_context(&self.item_id, cx);
        let images = image_loader(self.state.clone());
        let dense = self.dense;
        self.bodies
            .retain(|id, _| self.thread.comments.iter().any(|comment| &comment.id == id));
        for comment in &self.thread.comments {
            let source = SharedString::from(comment.body.clone());
            match self.bodies.get(&comment.id) {
                Some(view) => {
                    view.update(cx, |view, cx| view.set_source(source, context.clone(), cx));
                }
                None => {
                    let context = context.clone();
                    let images = images.clone();
                    let view =
                        cx.new(|cx| MarkdownView::new(source, context, Some(images), dense, cx));
                    self.bodies.insert(comment.id.clone(), view);
                }
            }
        }
    }

    /// Opens the composer and puts the caret in it (`autoFocus`).
    pub fn start_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reply.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextInput::new(cx)
                .multi_line(3, 10)
                .placeholder("Reply to this thread…")
        });
        let subscription = cx.subscribe(
            &input,
            |this: &mut Self, _input, event: &TextInputEvent, cx| match event {
                TextInputEvent::Changed => cx.notify(),
                TextInputEvent::Submit => this.send_reply(cx),
                TextInputEvent::Cancel => this.cancel_reply(cx),
            },
        );
        input.read(cx).focus(window);
        self.reply = Some(ReplyBox {
            input,
            busy: false,
            error: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn cancel_reply(&mut self, cx: &mut Context<Self>) {
        self.reply = None;
        cx.notify();
    }

    /// `ReplyBox.send`: nothing to send, or already sending, does nothing. The reply
    /// goes out trimmed; on failure the text stays where it was typed.
    fn send_reply(&mut self, cx: &mut Context<Self>) {
        let Some(reply) = self.reply.as_mut() else {
            return;
        };
        let body = reply.input.read(cx).text().trim().to_string();
        if body.is_empty() || reply.busy {
            return;
        }
        reply.busy = true;
        reply.error = None;
        let input = reply.input.clone();
        input.update(cx, |input, cx| input.set_disabled(true, cx));

        let task = self.state.update(cx, |state, cx| {
            state.reply_to_thread(&self.item_id, &self.thread.id, &body, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(reply) = this.reply.as_mut() else {
                    return;
                };
                reply.busy = false;
                reply
                    .input
                    .update(cx, |input, cx| input.set_disabled(false, cx));
                match result {
                    Ok(()) => {
                        this.reply = None;
                        cx.emit(ThreadEvent::Changed);
                    }
                    Err(error) => reply.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// `toggleResolved`: one request at a time.
    fn toggle_resolved(&mut self, cx: &mut Context<Self>) {
        if self.resolving {
            return;
        }
        self.resolving = true;
        self.resolve_error = None;
        let resolved = !self.thread.resolved;
        let task = self.state.update(cx, |state, cx| {
            state.set_thread_resolved(&self.item_id, &self.thread.id, resolved, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.resolving = false;
                match result {
                    Ok(()) => cx.emit(ThreadEvent::Changed),
                    Err(error) => this.resolve_error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The badges and the file reference above the comments. Away from the diff, the
    /// file is the only thing placing the thread - and an outdated one carries no line,
    /// deliberately, because the line it was left on means something else in the diff
    /// as it stands now.
    fn header(&self, cx: &App) -> Option<gpui::Div> {
        let thread = &self.thread;
        let path = thread.path.as_ref().filter(|_| !self.dense);
        if !(thread.resolved || thread.outdated || path.is_some()) {
            return None;
        }
        let colors = cx.theme().colors;
        let mut row = div()
            .mb(rpx(6.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(6.));
        if thread.resolved {
            row = row.child(
                Badge::new()
                    .tone(BadgeTone::Ok)
                    .child(Icon::new(IconName::Check).size(12.))
                    .child("Resolved"),
            );
        }
        if thread.outdated {
            row = row.child(Badge::new().tone(BadgeTone::Busy).child("Outdated"));
        }
        if let Some(path) = path {
            let location = match (thread.line, thread.start_line) {
                (Some(line), Some(start)) => format!("{path}:{start}-{line}"),
                (Some(line), None) => format!("{path}:{line}"),
                (None, _) => path.clone(),
            };
            row = row.child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(rpx(11.))
                    .text_color(colors.muted_foreground)
                    .child(location),
            );
        }
        Some(row)
    }

    fn comment(&self, comment: &PullComment, now: i64, cx: &App) -> gpui::Div {
        let colors = cx.theme().colors;
        let dense = self.dense;
        let mut avatar = Avatar::new(comment.author.name.clone());
        if !comment.author.avatar_url.is_empty() {
            avatar = avatar.src(comment.author.avatar_url.clone());
        }
        if dense {
            avatar = avatar.size(20.);
        }
        div()
            .flex()
            .gap(rpx(10.))
            .child(div().flex_none().child(avatar))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_size(rpx(if dense { 11.5 } else { 12. }))
                            .line_height(rpx(if dense { 17. } else { 18. }))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(rpx(4.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(comment.author.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.muted_foreground)
                                            .child(relative_time(&comment.created_at, now)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .mt(rpx(4.))
                            .children(self.bodies.get(&comment.id).cloned()),
                    ),
            )
    }

    fn reply_box(&self, reply: &ReplyBox, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;
        let empty = reply.input.read(cx).text().trim().is_empty();
        let busy = reply.busy;
        div()
            .mt(rpx(8.))
            .child(reply.input.clone())
            .children(reply.error.clone().map(|error| {
                div()
                    .mt(rpx(4.))
                    .text_size(rpx(11.5))
                    .text_color(colors.bad)
                    .child(error)
            }))
            .child(
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
                            .child("⌘↵ to send"),
                    )
                    .child(
                        Button::new("reply-cancel")
                            .size(ButtonSize::Sm)
                            .variant(ButtonVariant::Ghost)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_reply(cx)))
                            .child("Cancel"),
                    )
                    .child(
                        Button::new("reply-send")
                            .size(ButtonSize::Sm)
                            .variant(ButtonVariant::Default)
                            .disabled(empty || busy)
                            .on_click(cx.listener(|this, _, _, cx| this.send_reply(cx)))
                            .child(if busy { "Sending…" } else { "Reply" }),
                    ),
            )
    }
}

impl Render for ThreadCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let now = now_ms();
        let thread = &self.thread;
        let (opening, replies) = match thread.comments.split_first() {
            Some((opening, replies)) => (Some(opening), replies),
            None => (None, &[][..]),
        };

        let mut article = div().id("thread").font_family(UI_FONT);
        article = if self.dense {
            article
                .glass_quiet(cx)
                .m(rpx(6.))
                .rounded(rpx(radius::MD))
                .px(rpx(10.))
                .py(rpx(8.))
        } else {
            article.glass(cx).rounded(rpx(radius::LG)).p(rpx(12.))
        };
        // Open questions are what should catch the eye first.
        if thread.resolved {
            article = article.opacity(0.6);
        }

        article = article.children(self.header(cx));
        article = article.children(opening.map(|comment| self.comment(comment, now, cx)));

        if !replies.is_empty() {
            article = article.child(
                div()
                    .mt(rpx(8.))
                    .flex()
                    .flex_col()
                    .gap(rpx(8.))
                    .border_l_2()
                    .border_color(colors.border)
                    .pl(rpx(10.))
                    .children(replies.iter().map(|reply| self.comment(reply, now, cx))),
            );
        }

        if thread.can_reply || thread.can_resolve {
            let replying = self.reply.is_some();
            let resolving = self.resolving;
            let resolved = thread.resolved;
            article = article.child(
                div()
                    .mt(rpx(8.))
                    .flex()
                    .items_center()
                    .gap(rpx(6.))
                    .when(thread.can_reply && !replying, |row| {
                        row.child(
                            Button::new("reply")
                                .size(ButtonSize::Sm)
                                .variant(ButtonVariant::Ghost)
                                .icon(IconName::CornerDownRight)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start_reply(window, cx)),
                                )
                                .child("Reply"),
                        )
                    })
                    .when(thread.can_resolve, |row| {
                        row.child(
                            div().ml_auto().child(
                                Button::new("resolve")
                                    .size(ButtonSize::Sm)
                                    .variant(ButtonVariant::Ghost)
                                    .icon(if resolved {
                                        IconName::RotateCcw
                                    } else {
                                        IconName::Check
                                    })
                                    .loading(resolving)
                                    .disabled(resolving)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.toggle_resolved(cx)),
                                    )
                                    .child(if resolved { "Reopen" } else { "Resolve" }),
                            ),
                        )
                    }),
            );
        }

        if let Some(error) = self.resolve_error.clone() {
            article = article.child(
                div()
                    .mt(rpx(4.))
                    .text_size(rpx(11.5))
                    .text_color(colors.bad)
                    .child(error),
            );
        }

        if let Some(reply) = &self.reply {
            article = article.child(self.reply_box(reply, cx));
        }
        article
    }
}

/// What `@someone`, `#123` and a relative image source in a body of this pull request
/// should point at. Every signed-in account is offered to the images, not just the
/// item's, because a body can embed an image from another host we are signed in to
/// (the `markdownTargets` of PullView.tsx). Without the item in the deck, the context
/// is empty and the body still renders.
pub(crate) fn markdown_context(item_id: &str, cx: &App) -> MarkdownContext {
    let state = cx.global::<GlobalState>().0.read(cx);
    let Some(item) = state.find(item_id) else {
        return MarkdownContext::default();
    };
    let web_url = state
        .account(&item.account_id)
        .map(|account| account.web_url)
        .unwrap_or_default();
    let repo_root = repository_root_of(&item);
    let accounts: Vec<ImageAccount> = state.accounts().iter().map(ImageAccount::from).collect();
    MarkdownContext {
        autolink: Some(AutolinkContext {
            provider: item.provider,
            web_url,
            repo_root: repo_root.clone(),
        }),
        images: Some(reviewdeck_core::images::ImageContext {
            repo_root,
            accounts,
        }),
    }
}

/// The loader a markdown body asks for images that need the account's credential.
/// The state fetches and caches them and notifies when one arrives.
pub(crate) fn image_loader(state: Entity<AppState>) -> ImageLoader {
    Rc::new(move |account_id: &str, url: &str, cx: &mut App| {
        state.update(cx, |state, cx| {
            state.authenticated_image(account_id, url, cx)
        })
    })
}
