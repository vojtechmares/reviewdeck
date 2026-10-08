//! Port of src/renderer/src/components/ReviewCard.tsx.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use reviewdeck_core::model::{MyReviewState, ReviewItem};
use reviewdeck_core::time::{now_ms, relative_time};

use crate::ui::approval_badge::approval_badge;
use crate::ui::check_pill::{CheckPill, tone_color};
use crate::ui::components::avatar::Avatar;
use crate::ui::components::badge::{Badge, BadgeTone};
use crate::ui::components::button::with_alpha;
use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, radius, rpx};

/// `formatCount`: `-` for a number the host did not report, thousands as `1.2k`.
pub fn format_count(value: Option<u32>) -> String {
    match value {
        None => "-".to_string(),
        Some(value) if value < 1000 => value.to_string(),
        Some(value) => format!("{:.1}k", value as f64 / 1000.0),
    }
}

type SelectHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One review request in the deck list.
///
/// The list scrolls to keep the selection in view when j/k moves it (the TSX did that
/// from an effect on the card with `scrollIntoView`); that happens in the owner of the
/// list, which knows the card's index.
#[derive(IntoElement)]
pub struct ReviewCard {
    item: Arc<ReviewItem>,
    account_label: Option<SharedString>,
    selected: bool,
    on_select: SelectHandler,
}

impl ReviewCard {
    pub fn new(
        item: Arc<ReviewItem>,
        account_label: Option<SharedString>,
        selected: bool,
        on_select: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> ReviewCard {
        ReviewCard {
            item,
            account_label,
            selected,
            on_select: Rc::new(on_select),
        }
    }
}

impl RenderOnce for ReviewCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let item = self.item;
        let selected = self.selected;
        let on_select = self.on_select;
        let muted = colors.muted_foreground;

        let avatar = {
            let avatar = Avatar::new(item.author.name.clone()).size(26.);
            if item.author.avatar_url.is_empty() {
                avatar
            } else {
                avatar.src(item.author.avatar_url.clone())
            }
        };

        // repo, number and age, on one baseline.
        let meta = div()
            .flex()
            .items_center()
            .w_full()
            .text_size(rpx(11.5))
            .line_height(rpx(16.))
            .text_color(muted)
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div().flex_none().mr(rpx(6.)).mt(rpx(1.)).child(
                            Icon::provider(item.provider)
                                .size(12.)
                                .color(with_alpha(muted, 0.7)),
                        ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(item.repo.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .ml(rpx(6.))
                            .text_color(with_alpha(muted, muted.a * 0.6))
                            .child(format!("#{}", item.number)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .ml(rpx(6.))
                    .text_color(with_alpha(muted, muted.a * 0.7))
                    .child(relative_time(&item.updated_at, now_ms())),
            );

        let title = div()
            .mt(rpx(2.))
            .line_clamp(2)
            .text_size(rpx(13.))
            .line_height(rpx(18.))
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .child(item.title.clone());

        let my_state = match item.my_review_state {
            MyReviewState::Approved => Some((BadgeTone::Ok, IconName::CheckCheck, "You approved")),
            MyReviewState::ChangesRequested => {
                Some((BadgeTone::Bad, IconName::XOctagon, "Changes requested"))
            }
            MyReviewState::Commented => {
                Some((BadgeTone::Info, IconName::MessageSquare, "Commented"))
            }
            MyReviewState::Pending => None,
        };

        // Gaps are margins on the slots: `gap` on this wrapping row came out as
        // nothing whenever the account label shared a line with the badges.
        let slot = |element: AnyElement| {
            div()
                .flex_none()
                .mr(rpx(6.))
                .mb(rpx(6.))
                .child(element)
                .into_any_element()
        };
        let mut slots: Vec<AnyElement> = Vec::new();
        slots.push(slot(
            CheckPill::new(format!("pill-{}", item.id), &item.checks).into_any_element(),
        ));
        if item.draft {
            slots.push(slot(
                Badge::new()
                    .child(
                        Icon::new(IconName::GitPullRequestArrow)
                            .size(12.)
                            .color(muted),
                    )
                    .child("Draft")
                    .into_any_element(),
            ));
        }
        if let Some((tone, icon, label)) = my_state {
            slots.push(slot(
                Badge::new()
                    .tone(tone)
                    .child(Icon::new(icon).size(12.).color(tone_color(tone, cx)))
                    .child(label)
                    .into_any_element(),
            ));
        }
        slots.push(slot(
            approval_badge(
                &item.approvals,
                SharedString::from(format!("approvals-{}", item.id)),
                cx,
            )
            .into_any_element(),
        ));
        if let Some(files) = item.changed_files {
            slots.push(slot(
                Badge::new()
                    .child(Icon::new(IconName::FileDiff).size(12.).color(muted))
                    .child(format_count(Some(files)))
                    .child(
                        div()
                            .text_color(colors.ok)
                            .child(format!("+{}", format_count(item.additions))),
                    )
                    .child(
                        div()
                            .text_color(colors.bad)
                            .child(format!("−{}", format_count(item.deletions))),
                    )
                    .into_any_element(),
            ));
        }
        if let Some(label) = self.account_label {
            slots.push(
                div()
                    .ml_auto()
                    .mb(rpx(6.))
                    .max_w(rpx(112.))
                    .truncate()
                    .text_size(rpx(10.5))
                    .text_color(with_alpha(muted, muted.a * 0.8))
                    .child(label)
                    .into_any_element(),
            );
        }
        let badges = div()
            .mt(rpx(8.))
            .mb(rpx(-6.))
            .flex()
            .flex_wrap()
            .items_center()
            .children(slots);

        let body = div()
            .flex()
            .items_start()
            .gap(rpx(10.))
            .child(div().mt(rpx(2.)).flex_none().child(avatar))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(meta)
                    .child(title)
                    .child(badges),
            );

        // The TSX card is a `<button>`: a tab stop that Enter and Space activate
        // (gpui turns those into a click on a focused element). The focus handle
        // is kept per card, under the card's id, as the list rebuilds cards freely.
        let focus = window.use_keyed_state(
            SharedString::from(format!("card-focus-{}", item.id)),
            cx,
            |_, cx| cx.focus_handle().tab_stop(true),
        );
        let focus = focus.read(cx).clone();
        let ring = colors.ring;
        let border = colors.border;
        let surface_muted = colors.surface_muted;
        let card = div()
            .id(SharedString::from(format!("card-{}", item.id)))
            .relative()
            .w_full()
            .rounded(rpx(radius::LG))
            .border_1()
            .px(rpx(12.))
            .py(rpx(10.))
            .cursor_pointer()
            .track_focus(&focus)
            .focus(move |s| s.border_color(ring))
            .when(selected, |d| {
                d.border_color(colors.border_strong)
                    .bg(colors.surface_strong)
                    // The `inset 0 1px 0 var(--highlight)` lip of the selected card.
                    .child(
                        div()
                            .absolute()
                            .top(rpx(0.))
                            .left(rpx(10.))
                            .right(rpx(10.))
                            .h(rpx(1.))
                            .bg(colors.highlight),
                    )
            })
            .when(!selected, |d| {
                d.border_color(gpui::transparent_black())
                    .hover(move |s| s.border_color(border).bg(surface_muted))
            })
            .on_click(move |event, window, cx| on_select(event, window, cx))
            .child(body);
        #[cfg(test)]
        let card = card.debug_selector({
            let id = item.id.clone();
            move || format!("card:{id}")
        });
        card
    }
}

#[cfg(test)]
mod tests {
    use super::format_count;

    #[test]
    fn counts_are_dashes_digits_or_thousands() {
        assert_eq!(format_count(None), "-");
        assert_eq!(format_count(Some(0)), "0");
        assert_eq!(format_count(Some(999)), "999");
        assert_eq!(format_count(Some(1000)), "1.0k");
        assert_eq!(format_count(Some(12_345)), "12.3k");
    }
}
