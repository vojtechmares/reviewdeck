//! Embedded lucide icons and provider logos; port of src/renderer/src/components/ProviderIcon.tsx.
//!
//! Every icon the renderer imports from `lucide-react` (lucide 1.31.0, ISC, see
//! `assets/icons/LICENSE-lucide`) and the four provider marks are compiled into the binary
//! with `include_bytes!`, served through [`Assets`]. Icons are alpha masks: gpui fills them
//! with the element's text colour, so `stroke="currentColor"` and the provider fills both
//! take whatever colour the view asks for.
//!
//! Asset paths are always `icons/<name>.svg`. gpui parses `img(..)` sources that look like
//! URIs as remote, but `svg().path(..)` always goes to the asset source, so these paths are
//! safe for [`Icon`].

use std::borrow::Cow;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, App, AssetSource, Hsla, IntoElement, RenderOnce, Result, SharedString,
    Styled, Transformation, Window, percentage, svg,
};
use reviewdeck_core::model::ProviderKind;

use super::theme::{ActiveTheme, rpx};

/// Expands to the `(path, bytes)` table, so each file is named once.
macro_rules! embedded {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../../assets/", $path)) as &[u8])),*]
    };
}

/// Every file the app can ask the asset source for.
static ASSETS: &[(&str, &[u8])] = embedded![
    "icons/triangle-alert.svg",
    "icons/check.svg",
    "icons/check-check.svg",
    "icons/circle-check.svg",
    "icons/chevron-down.svg",
    "icons/chevron-right.svg",
    "icons/circle-dashed.svg",
    "icons/clipboard-copy.svg",
    "icons/columns-2.svg",
    "icons/corner-down-right.svg",
    "icons/external-link.svg",
    "icons/file-diff.svg",
    "icons/file-minus-2.svg",
    "icons/file-plus-2.svg",
    "icons/file-symlink.svg",
    "icons/filter.svg",
    "icons/git-branch.svg",
    "icons/git-pull-request-arrow.svg",
    "icons/git-pull-request-draft.svg",
    "icons/inbox.svg",
    "icons/info.svg",
    "icons/lightbulb.svg",
    "icons/loader-circle.svg",
    "icons/message-square.svg",
    "icons/message-square-plus.svg",
    "icons/message-square-warning.svg",
    "icons/octagon-alert.svg",
    "icons/pencil.svg",
    "icons/plus.svg",
    "icons/refresh-cw.svg",
    "icons/rotate-ccw.svg",
    "icons/rows-3.svg",
    "icons/search.svg",
    "icons/send.svg",
    "icons/settings-2.svg",
    "icons/trash-2.svg",
    "icons/user-check.svg",
    "icons/user-round-plus.svg",
    "icons/x.svg",
    "icons/circle-x.svg",
    "icons/octagon-x.svg",
    "icons/provider-github.svg",
    "icons/provider-gitlab.svg",
    "icons/provider-forgejo.svg",
    "icons/provider-bitbucket.svg",
];

/// The asset source the application is started with: `Application::new().with_assets(Assets)`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // An unknown path is not an error: `svg()` then paints nothing.
        Ok(ASSETS
            .iter()
            .find(|(known, _)| *known == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter(|(known, _)| known.starts_with(path))
            .map(|(known, _)| SharedString::from(*known))
            .collect())
    }
}

/// The lucide icons the renderer uses. Names follow the `lucide-react` exports, so an
/// `import { Foo } from 'lucide-react'` becomes `IconName::Foo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    AlertTriangle,
    Check,
    CheckCheck,
    CheckCircle2,
    ChevronDown,
    ChevronRight,
    CircleDashed,
    ClipboardCopy,
    Columns2,
    CornerDownRight,
    ExternalLink,
    FileDiff,
    FileMinus2,
    FilePlus2,
    FileSymlink,
    Filter,
    GitBranch,
    GitPullRequestArrow,
    GitPullRequestDraft,
    Inbox,
    Info,
    Lightbulb,
    Loader2,
    MessageSquare,
    MessageSquarePlus,
    MessageSquareWarning,
    OctagonAlert,
    Pencil,
    Plus,
    RefreshCw,
    RotateCcw,
    Rows3,
    Search,
    Send,
    Settings2,
    Trash2,
    TriangleAlert,
    UserCheck,
    UserRoundPlus,
    X,
    XCircle,
    XOctagon,
}

impl IconName {
    /// The asset path of this icon's SVG.
    pub fn path(self) -> &'static str {
        match self {
            Self::AlertTriangle | Self::TriangleAlert => "icons/triangle-alert.svg",
            Self::Check => "icons/check.svg",
            Self::CheckCheck => "icons/check-check.svg",
            Self::CheckCircle2 => "icons/circle-check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::CircleDashed => "icons/circle-dashed.svg",
            Self::ClipboardCopy => "icons/clipboard-copy.svg",
            Self::Columns2 => "icons/columns-2.svg",
            Self::CornerDownRight => "icons/corner-down-right.svg",
            Self::ExternalLink => "icons/external-link.svg",
            Self::FileDiff => "icons/file-diff.svg",
            Self::FileMinus2 => "icons/file-minus-2.svg",
            Self::FilePlus2 => "icons/file-plus-2.svg",
            Self::FileSymlink => "icons/file-symlink.svg",
            Self::Filter => "icons/filter.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::GitPullRequestArrow => "icons/git-pull-request-arrow.svg",
            Self::GitPullRequestDraft => "icons/git-pull-request-draft.svg",
            Self::Inbox => "icons/inbox.svg",
            Self::Info => "icons/info.svg",
            Self::Lightbulb => "icons/lightbulb.svg",
            Self::Loader2 => "icons/loader-circle.svg",
            Self::MessageSquare => "icons/message-square.svg",
            Self::MessageSquarePlus => "icons/message-square-plus.svg",
            Self::MessageSquareWarning => "icons/message-square-warning.svg",
            Self::OctagonAlert => "icons/octagon-alert.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::Plus => "icons/plus.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::RotateCcw => "icons/rotate-ccw.svg",
            Self::Rows3 => "icons/rows-3.svg",
            Self::Search => "icons/search.svg",
            Self::Send => "icons/send.svg",
            Self::Settings2 => "icons/settings-2.svg",
            Self::Trash2 => "icons/trash-2.svg",
            Self::UserCheck => "icons/user-check.svg",
            Self::UserRoundPlus => "icons/user-round-plus.svg",
            Self::X => "icons/x.svg",
            Self::XCircle => "icons/circle-x.svg",
            Self::XOctagon => "icons/octagon-x.svg",
        }
    }

    /// An [`Icon`] element for this icon at the default size (16px).
    pub fn icon(self) -> Icon {
        Icon::new(self)
    }
}

/// The asset path of a provider's mark.
pub fn provider_path(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Github => "icons/provider-github.svg",
        ProviderKind::Gitlab => "icons/provider-gitlab.svg",
        ProviderKind::Forgejo => "icons/provider-forgejo.svg",
        ProviderKind::Bitbucket => "icons/provider-bitbucket.svg",
    }
}

/// A 16px lucide icon, or a provider mark, painted in one colour.
///
/// The colour defaults to the theme's foreground. It is set on the SVG itself because
/// `svg()` only paints with a colour given to it directly.
#[derive(IntoElement)]
pub struct Icon {
    path: &'static str,
    size: f32,
    color: Option<Hsla>,
    spin: bool,
}

impl Icon {
    pub fn new(name: IconName) -> Icon {
        Icon {
            path: name.path(),
            size: 16.,
            color: None,
            spin: false,
        }
    }

    /// The provider's mark, which `ProviderIcon` draws at `size-3.5` (14px) by default.
    pub fn provider(kind: ProviderKind) -> Icon {
        Icon {
            path: provider_path(kind),
            size: 14.,
            color: None,
            spin: false,
        }
    }

    /// Side length in CSS pixels (`size-4` is 16).
    pub fn size(mut self, css_px: f32) -> Icon {
        self.size = css_px;
        self
    }

    pub fn color(mut self, color: Hsla) -> Icon {
        self.color = Some(color);
        self
    }

    /// Turns the icon into a spinner: one full turn every 900ms, the `.spin` class.
    pub fn spin(mut self) -> Icon {
        self.spin = true;
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(cx.theme().colors.foreground);
        let icon = svg()
            .path(self.path)
            .size(rpx(self.size))
            .flex_none()
            .text_color(color);
        if self.spin {
            icon.with_animation(
                "rd-spin",
                Animation::new(Duration::from_millis(900)).repeat(),
                |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
            )
            .into_any_element()
        } else {
            icon.into_any_element()
        }
    }
}
