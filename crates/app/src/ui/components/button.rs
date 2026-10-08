//! Port of src/renderer/src/components/ui/button.tsx.
//!
//! Variants and sizes are the TypeScript ones. `loading` replaces the leading icon with a
//! spinner and stops clicks, which the TSX buttons do by hand in each view.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder,
};

use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, Colors, radius, rpx};

use super::tooltip::tooltip;

/// `variant` on the TSX button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// `bg-primary`: the call to action.
    Default,
    /// `bg-surface-strong` with a border. The TSX default.
    #[default]
    Secondary,
    Ghost,
    Outline,
    Success,
    Danger,
    Subtle,
}

/// `size` on the TSX button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// `h-7 px-2.5 text-[12px] gap-1.5 rounded-md`.
    Sm,
    /// `h-8.5 px-3.5 text-[13px] gap-2 rounded-lg`. The TSX default.
    #[default]
    Md,
    /// `h-10 px-5 text-[14px] gap-2 rounded-lg`.
    Lg,
    /// `h-8 w-8 rounded-lg`, for a single icon.
    Icon,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The button, built like the TSX one and then placed as an element.
///
/// ```ignore
/// Button::new("refresh")
///     .variant(ButtonVariant::Ghost)
///     .size(ButtonSize::Icon)
///     .icon(IconName::RefreshCw)
///     .tooltip("Refresh (⌘R)")
///     .on_click(|_, window, cx| { /* ... */ })
/// ```
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    variant: ButtonVariant,
    size: ButtonSize,
    icon: Option<IconName>,
    loading: bool,
    disabled: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
    children: Vec<AnyElement>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Button {
        Button {
            id: id.into(),
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            icon: None,
            loading: false,
            disabled: false,
            tooltip: None,
            on_click: None,
            children: Vec::new(),
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Button {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Button {
        self.size = size;
        self
    }

    /// A leading icon, drawn before the children.
    pub fn icon(mut self, icon: IconName) -> Button {
        self.icon = Some(icon);
        self
    }

    /// An icon-only button: `size(ButtonSize::Icon)` with `icon(..)` and no children.
    pub fn icon_only(mut self, icon: IconName) -> Button {
        self.size = ButtonSize::Icon;
        self.icon = Some(icon);
        self
    }

    /// Shows a spinner in place of the icon and ignores clicks.
    pub fn loading(mut self, loading: bool) -> Button {
        self.loading = loading;
        self
    }

    /// `disabled:pointer-events-none disabled:opacity-45`.
    pub fn disabled(mut self, disabled: bool) -> Button {
        self.disabled = disabled;
        self
    }

    /// A hover hint, shown after the standard delay.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Button {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Button {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// The colours one variant paints in: rest, hover and border.
struct VariantColors {
    background: Hsla,
    foreground: Hsla,
    border: Hsla,
    hover_background: Hsla,
    hover_foreground: Hsla,
    shadow: bool,
}

fn variant_colors(variant: ButtonVariant, colors: &Colors) -> VariantColors {
    let plain = |background, foreground, border, hover_background| VariantColors {
        background,
        foreground,
        border,
        hover_background,
        hover_foreground: foreground,
        shadow: false,
    };
    let transparent = gpui::transparent_black();
    match variant {
        ButtonVariant::Default => VariantColors {
            background: colors.primary,
            foreground: colors.primary_foreground,
            border: transparent,
            // `hover:opacity-90`
            hover_background: with_alpha(colors.primary, 0.9),
            hover_foreground: colors.primary_foreground,
            shadow: true,
        },
        ButtonVariant::Secondary => VariantColors {
            background: colors.surface_strong,
            foreground: colors.foreground,
            border: colors.border,
            hover_background: colors.accent,
            hover_foreground: colors.foreground,
            shadow: false,
        },
        ButtonVariant::Ghost => VariantColors {
            background: transparent,
            foreground: colors.muted_foreground,
            border: transparent,
            hover_background: colors.accent,
            hover_foreground: colors.foreground,
            shadow: false,
        },
        ButtonVariant::Outline => plain(
            transparent,
            colors.foreground,
            colors.border_strong,
            colors.accent,
        ),
        ButtonVariant::Success => VariantColors {
            background: colors.ok_soft,
            foreground: colors.ok,
            border: with_alpha(colors.ok, 0.3),
            hover_background: with_alpha(colors.ok, 0.25),
            hover_foreground: colors.ok,
            shadow: false,
        },
        ButtonVariant::Danger => VariantColors {
            background: colors.bad_soft,
            foreground: colors.bad,
            border: with_alpha(colors.bad, 0.3),
            hover_background: with_alpha(colors.bad, 0.25),
            hover_foreground: colors.bad,
            shadow: false,
        },
        ButtonVariant::Subtle => VariantColors {
            background: colors.muted,
            foreground: colors.foreground,
            border: transparent,
            hover_background: colors.accent,
            hover_foreground: colors.foreground,
            shadow: false,
        },
    }
}

/// `color/alpha`, the Tailwind `bg-ok/30` form.
pub(crate) fn with_alpha(colour: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..colour }
}

/// Height, horizontal padding, font size, gap, radius, icon size.
struct Metrics {
    height: f32,
    padding_x: f32,
    font_size: f32,
    gap: f32,
    radius: f32,
    icon: f32,
}

fn metrics(size: ButtonSize) -> Metrics {
    match size {
        ButtonSize::Sm => Metrics {
            height: 28.,
            padding_x: 10.,
            font_size: 12.,
            gap: 6.,
            radius: radius::MD,
            icon: 14.,
        },
        ButtonSize::Md => Metrics {
            height: 34.,
            padding_x: 14.,
            font_size: 13.,
            gap: 8.,
            radius: radius::LG,
            icon: 16.,
        },
        ButtonSize::Lg => Metrics {
            height: 40.,
            padding_x: 20.,
            font_size: 14.,
            gap: 8.,
            radius: radius::LG,
            icon: 16.,
        },
        ButtonSize::Icon => Metrics {
            height: 32.,
            padding_x: 0.,
            font_size: 13.,
            gap: 0.,
            radius: radius::LG,
            icon: 16.,
        },
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let paint = variant_colors(self.variant, &colors);
        let sizing = metrics(self.size);
        let icon_colour = paint.foreground;
        let inert = self.disabled || self.loading;
        let icon_size = sizing.icon;

        let leading: Option<AnyElement> = if self.loading {
            Some(
                Icon::new(IconName::Loader2)
                    .size(icon_size)
                    .color(icon_colour)
                    .spin()
                    .into_any_element(),
            )
        } else {
            self.icon.map(|icon| {
                Icon::new(icon)
                    .size(icon_size)
                    .color(icon_colour)
                    .into_any_element()
            })
        };

        let hover_background = paint.hover_background;
        let hover_foreground = paint.hover_foreground;

        let mut element = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .whitespace_nowrap()
            .font_weight(FontWeight::MEDIUM)
            .h(rpx(sizing.height))
            .px(rpx(sizing.padding_x))
            .gap(rpx(sizing.gap))
            .rounded(rpx(sizing.radius))
            .text_size(rpx(sizing.font_size))
            .border_1()
            .border_color(paint.border)
            .bg(paint.background)
            .text_color(paint.foreground)
            .when(paint.shadow, |d| {
                d.shadow(vec![gpui::BoxShadow {
                    color: colors.overlay_shadow,
                    offset: gpui::point(gpui::px(0.), gpui::px(1.)),
                    blur_radius: gpui::px(2.),
                    spread_radius: gpui::px(0.),
                }])
            })
            .when(inert, |d| d.opacity(0.45).cursor_default())
            .when(!inert, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(hover_background).text_color(hover_foreground))
            })
            .when_some(leading, |d, icon| d.child(icon))
            .children(self.children);

        if let Some(text) = self.tooltip {
            element = element.tooltip(tooltip(text));
        }

        if let Some(handler) = self.on_click.filter(|_| !inert) {
            element = element.on_click(move |event, window, cx| handler(event, window, cx));
        }

        // The TSX button eats pointer events while disabled; a stray press must not reach
        // the element behind it.
        element.when(inert, |d| {
            d.occlude().on_mouse_down(MouseButton::Left, |_, _, _| {})
        })
    }
}
