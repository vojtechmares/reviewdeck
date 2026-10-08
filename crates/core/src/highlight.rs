//! Syntax highlighting: putting syntax tokens back on diff lines, and the
//! highlighter itself.
//!
//! A port of src/shared/highlight.ts (the zip, the background rules, language
//! detection) and of src/renderer/src/lib/highlight.ts (what the renderer did with
//! Shiki), here done with syntect's parser over two-face's grammars and the same two
//! themes Shiki was given.
//!
//! A diff is two interleaved versions of a file, and neither is valid source on its
//! own, so each side of a hunk is reconstructed and tokenized separately: the old
//! side from context and deleted lines, the new side from context and added ones.
//! The highlighter hands tokens back already grouped per line, which is what makes
//! putting them back a zip rather than character-offset arithmetic.
//!
//! The zip is generic over whatever a token turns out to be, and is tested
//! directly. That matters more here than almost anywhere else in the app, because
//! wrong output still looks like syntax highlighting.
//!
//! Accepted limitation: a hunk that starts inside a multi-line construct can
//! mis-tokenize, because the hunk is all the text there is. This is what the hosts
//! themselves do, and it goes away if whole files ever become readable.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, OnceLock};

use serde::Deserialize;
use syntect::parsing::{
    BasicScopeStackOp, ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet,
};

use crate::color::parse_color;
use crate::diff::{DiffHunk, DiffLineKind, DiffSide};
use crate::model::Side;

// ---------------------------------------------------------------------------
// Diff sides and the zip
// ---------------------------------------------------------------------------

/// The lines of a hunk that exist on one side, in order, as indices into
/// [`DiffHunk::lines`].
///
/// Meta lines - the no-trailing-newline marker - are not source and would shift
/// every token line after them by one.
pub fn side_lines(hunk: &DiffHunk, side: DiffSide) -> Vec<usize> {
    let changed = match side {
        Side::Old => DiffLineKind::Del,
        Side::New => DiffLineKind::Add,
    };
    hunk.lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind == DiffLineKind::Context || line.kind == changed)
        .map(|(index, _)| index)
        .collect()
}

/// The source text one side of a hunk represents, for the highlighter to read.
pub fn side_text(hunk: &DiffHunk, side: DiffSide) -> String {
    side_lines(hunk, side)
        .into_iter()
        .map(|index| hunk.lines[index].content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Token lines back onto the diff lines they came from: one slot per line of the
/// hunk, by index - which is what both the unified and the split layouts address a
/// line by, the split one using the same index for a context line in both columns.
///
/// A line the highlighter produced nothing for is `None`, so it renders plain
/// rather than rendering wrong.
pub fn hunk_tokens<T>(
    hunk: &DiffHunk,
    old_side: Vec<Vec<T>>,
    new_side: Vec<Vec<T>>,
) -> Vec<Option<Vec<T>>> {
    let mut tokens: Vec<Option<Vec<T>>> = hunk.lines.iter().map(|_| None).collect();

    // Old first, so a context line keeps the new side's tokens: that is the text the
    // reader is looking at, and the two can differ around a changed line.
    for (side, produced) in [(Side::Old, old_side), (Side::New, new_side)] {
        for (index, token_line) in side_lines(hunk, side).into_iter().zip(produced) {
            tokens[index] = Some(token_line);
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Backgrounds
// ---------------------------------------------------------------------------

/*
 * The scopes a theme paints a background behind, and which languages can produce
 * them.
 *
 * Shiki emitted foregrounds only - `codeToTokens` carries no background in any mode
 * - so the handful of theme rules that set one would render with a foreground
 * chosen for a background nothing paints. In `github-dark-default` the
 * carriage-return marker is `#f0f6fc` on `#ff7b72`; without the red behind it, it
 * is white text on the code column. The highlighter here resolves foregrounds the
 * same way, and paints backgrounds by the same separate rule.
 *
 * Recovering it from the foreground does not work: three of the five backgrounds in
 * each theme share their foreground with rules that set none, so `#116329` is
 * `markup.inserted` and also `entity.name.tag`, and keying off the colour paints
 * green behind every JSX tag name. The scopes a token actually matched are the only
 * sound key, and asking for them costs extra, which is why it is asked for by
 * language rather than always.
 */

/// One theme rule that sets a background, flattened to a single scope selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundRule {
    pub selector: String,
    pub background: String,
}

/// A token colour rule, in the shape a theme's JSON carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ThemeTokenRule {
    #[serde(default)]
    pub scope: Option<ThemeScope>,
    #[serde(default)]
    pub settings: Option<ThemeRuleSettings>,
}

/// `scope` is a string or a list of them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ThemeScope {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeRuleSettings {
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub font_style: Option<String>,
}

impl ThemeTokenRule {
    /// The rule's scopes as the theme lists them: a string scope is one entry, as
    /// `typeof rule.scope === 'string' ? [rule.scope] : (rule.scope ?? [])` had it.
    pub fn scopes(&self) -> impl Iterator<Item = &str> {
        let list: &[String] = match &self.scope {
            None => &[],
            Some(ThemeScope::One(scope)) => std::slice::from_ref(scope),
            Some(ThemeScope::Many(scopes)) => scopes,
        };
        list.iter().map(String::as_str)
    }
}

/// Languages whose grammars can emit those scopes at all.
///
/// `markup.*` and `carriage-return` come from the diff and markdown grammars and
/// nowhere else, and the diff view tokenizes each side with the *file's own*
/// language rather than with the `diff` grammar - so a reviewed `.ts` file cannot
/// produce them however it is spelled. What is left is a reviewed patch or markdown
/// file, and a fenced block tagged as one.
const BACKGROUND_LANGUAGES: &[&str] = &["diff", "markdown", "mdx"];

/// Whether tokens of this language are worth asking the extra question about.
pub fn paints_backgrounds(language: &str) -> bool {
    BACKGROUND_LANGUAGES.contains(&language)
}

/// A theme's background-setting rules, one entry per scope selector.
pub fn background_rules(token_colors: &[ThemeTokenRule]) -> Vec<BackgroundRule> {
    let mut rules = Vec::new();
    for rule in token_colors {
        let Some(background) = rule
            .settings
            .as_ref()
            .and_then(|settings| settings.background.as_deref())
            .filter(|background| !background.is_empty())
        else {
            continue;
        };
        for selector in rule.scopes() {
            rules.push(BackgroundRule {
                selector: selector.to_string(),
                background: background.to_string(),
            });
        }
    }
    rules
}

/// Whether a TextMate selector matches a scope: it is the scope, or a
/// dot-separated prefix of it.
fn selector_matches(selector: &str, scope: &str) -> bool {
    scope
        .strip_prefix(selector)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// The index of the rule [`background_for`] picks among those matching one scope.
fn best_background_rule(rules: &[BackgroundRule], scope: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, rule) in rules.iter().enumerate() {
        if best.is_some_and(|best| rule.selector.len() <= rules[best].selector.len()) {
            continue;
        }
        if selector_matches(&rule.selector, scope) {
            best = Some(index);
        }
    }
    best
}

/// The better of two [`best_background_rule`] picks: the longer selector, and the
/// earlier rule on a tie - which is the order [`background_for`] walks them in.
fn better_background_rule(
    rules: &[BackgroundRule],
    one: Option<usize>,
    other: Option<usize>,
) -> Option<usize> {
    match (one, other) {
        (Some(a), Some(b)) => {
            let (la, lb) = (rules[a].selector.len(), rules[b].selector.len());
            Some(if la > lb || (la == lb && a < b) { a } else { b })
        }
        (one, other) => one.or(other),
    }
}

/// The background a token's matched scopes call for, or nothing - which is what
/// almost every token gets.
///
/// Matching is TextMate's: a selector matches a scope it is a dot-separated prefix
/// of, so `markup.deleted` matches `markup.deleted.diff`, and the longest selector
/// to match wins. A token that matched no background-setting rule keeps the
/// translucent row behind it, which is the whole point of the base film.
pub fn background_for<'a, S: AsRef<str>>(
    rules: &'a [BackgroundRule],
    scopes: &[S],
) -> Option<&'a str> {
    let mut best: Option<&BackgroundRule> = None;
    for rule in rules {
        if best.is_some_and(|best| rule.selector.len() <= best.selector.len()) {
            continue;
        }
        if scopes
            .iter()
            .any(|scope| selector_matches(&rule.selector, scope.as_ref()))
        {
            best = Some(rule);
        }
    }
    best.map(|rule| rule.background.as_str())
}

// ---------------------------------------------------------------------------
// Language detection
// ---------------------------------------------------------------------------

/// What to tokenise a file as, by extension (sorted, for the binary search).
///
/// Owned code rather than a library's guess, because the awkward cases are the ones
/// that matter here: compound extensions, files that carry no extension at all, and
/// the configuration formats that turn up in every infrastructure repository and get
/// left out of curated bundles.
///
/// A name that is not in here resolves to nothing and renders plain. That is a
/// deliberate boundary: guessing a language from an unknown extension would tokenise
/// a file as something it is not, which reads worse than not colouring it.
///
/// The values are the language ids the TypeScript gave Shiki; [`SYNTAXES`] maps
/// them onto grammars.
const BY_EXTENSION: &[(&str, &str)] = &[
    ("adoc", "asciidoc"),
    ("ahk", "ahk"),
    ("applescript", "applescript"),
    ("as", "actionscript-3"),
    ("asm", "asm"),
    ("astro", "astro"),
    ("awk", "awk"),
    ("bash", "shellscript"),
    ("bat", "bat"),
    ("bicep", "bicep"),
    ("c", "c"),
    ("cc", "cpp"),
    ("cfg", "ini"),
    ("cjs", "javascript"),
    ("clj", "clojure"),
    ("cljc", "clojure"),
    ("cljs", "clojure"),
    ("cmake", "cmake"),
    ("cmd", "bat"),
    ("cob", "cobol"),
    ("coffee", "coffee"),
    ("conf", "ini"),
    ("cpp", "cpp"),
    ("cs", "csharp"),
    ("cshtml", "razor"),
    ("css", "css"),
    ("csv", "csv"),
    ("cts", "typescript"),
    ("cxx", "cpp"),
    ("d", "d"),
    ("dart", "dart"),
    ("diff", "diff"),
    ("ejs", "erb"),
    ("elm", "elm"),
    ("erl", "erlang"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("fish", "fish"),
    ("fs", "fsharp"),
    ("fsx", "fsharp"),
    ("gemspec", "ruby"),
    ("gleam", "gleam"),
    ("glsl", "glsl"),
    ("go", "go"),
    ("gql", "graphql"),
    ("gradle", "groovy"),
    ("graphql", "graphql"),
    ("groovy", "groovy"),
    ("h", "c"),
    ("handlebars", "handlebars"),
    ("haxe", "haxe"),
    ("hbs", "handlebars"),
    ("hcl", "hcl"),
    ("hh", "cpp"),
    ("hlsl", "hlsl"),
    ("hpp", "cpp"),
    ("hrl", "erlang"),
    ("hs", "haskell"),
    ("htm", "html"),
    ("html", "html"),
    ("http", "http"),
    ("hxx", "cpp"),
    ("ini", "ini"),
    ("java", "java"),
    ("jl", "julia"),
    ("js", "javascript"),
    ("json", "json"),
    ("json5", "json5"),
    ("jsonc", "jsonc"),
    ("jsonl", "json"),
    ("jsx", "jsx"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("latex", "latex"),
    ("less", "less"),
    ("liquid", "liquid"),
    ("lua", "lua"),
    ("m", "objective-c"),
    ("markdown", "markdown"),
    ("md", "markdown"),
    ("mdx", "mdx"),
    ("mjs", "javascript"),
    ("mk", "make"),
    ("ml", "ocaml"),
    ("mli", "ocaml"),
    ("mm", "objective-cpp"),
    ("move", "move"),
    ("mts", "typescript"),
    ("nim", "nim"),
    ("nix", "nix"),
    ("odin", "odin"),
    ("pas", "pascal"),
    ("patch", "diff"),
    ("php", "php"),
    ("pl", "perl"),
    ("plist", "xml"),
    ("pm", "perl"),
    ("powershell", "powershell"),
    ("pp", "puppet"),
    ("prisma", "prisma"),
    ("pro", "prolog"),
    ("properties", "properties"),
    ("proto", "proto"),
    ("ps1", "powershell"),
    ("psm1", "powershell"),
    ("pug", "pug"),
    ("purs", "purescript"),
    ("py", "python"),
    ("pyi", "python"),
    ("r", "r"),
    ("rake", "ruby"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("rst", "rst"),
    ("s", "asm"),
    ("sass", "sass"),
    ("sc", "scala"),
    ("scala", "scala"),
    ("scm", "scheme"),
    ("scss", "scss"),
    ("sh", "shellscript"),
    ("sol", "solidity"),
    ("sql", "sql"),
    ("styl", "stylus"),
    ("svelte", "svelte"),
    ("svg", "xml"),
    ("swift", "swift"),
    ("tcl", "tcl"),
    ("tex", "latex"),
    ("tf", "hcl"),
    ("tfvars", "hcl"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsv", "csv"),
    ("tsx", "tsx"),
    ("twig", "twig"),
    ("v", "v"),
    ("vb", "vb"),
    ("vhd", "vhdl"),
    ("vhdl", "vhdl"),
    ("vim", "viml"),
    ("vue", "vue"),
    ("wat", "wasm"),
    ("wgsl", "wgsl"),
    ("xml", "xml"),
    ("xsd", "xml"),
    ("xsl", "xml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("zig", "zig"),
    ("zsh", "shellscript"),
];

/// Files that carry no extension at all but are still unambiguous.
const BY_FILENAME: &[(&str, &str)] = &[
    (".bash_profile", "shellscript"),
    (".bashrc", "shellscript"),
    (".gitconfig", "ini"),
    (".profile", "shellscript"),
    (".zshrc", "shellscript"),
    ("cmakelists.txt", "cmake"),
    ("brewfile", "ruby"),
    ("dockerfile", "docker"),
    ("gemfile", "ruby"),
    ("gnumakefile", "make"),
    ("jenkinsfile", "groovy"),
    ("justfile", "make"),
    ("makefile", "make"),
    ("podfile", "ruby"),
    ("rakefile", "ruby"),
    ("vagrantfile", "ruby"),
];

fn by_extension(extension: &str) -> Option<&'static str> {
    BY_EXTENSION
        .binary_search_by(|(key, _)| (*key).cmp(extension))
        .ok()
        .map(|at| BY_EXTENSION[at].1)
}

fn by_filename(name: &str) -> Option<&'static str> {
    BY_FILENAME
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, language)| *language)
}

/// Every language [`language_for`] can name, sorted and without repeats, so the
/// grammar mapping can be checked against it.
pub static HIGHLIGHT_LANGUAGES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut languages: Vec<&'static str> = BY_EXTENSION
        .iter()
        .chain(BY_FILENAME)
        .map(|(_, language)| *language)
        .collect();
    languages.sort_unstable();
    languages.dedup();
    languages
});

/// The language to tokenise a path as, or `None` when this app has no mapping for
/// it - in which case the file renders as plain text rather than as an error.
pub fn language_for(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if name.is_empty() {
        return None;
    }

    if let Some(language) = by_filename(&name) {
        return Some(language);
    }

    // `Dockerfile.dev` is still a Dockerfile, and `Makefile.common` still a makefile.
    //
    // A name with no dot at all is cut one character short, as
    // `name.slice(0, name.indexOf('.'))` cuts it when `indexOf` says -1, so
    // `Makefiles` reads as a makefile here exactly as it did there.
    let base = match name.find('.') {
        Some(dot) => &name[..dot],
        None => name
            .char_indices()
            .last()
            .map_or("", |(last, _)| &name[..last]),
    };
    if !base.is_empty()
        && let Some(language) = by_filename(base)
    {
        return Some(language);
    }

    // The last extension wins, so `component.test.ts` and `types.d.ts` resolve.
    let dot = name.rfind('.')?;
    if dot == 0 {
        return None;
    }
    by_extension(&name[dot + 1..])
}

// ---------------------------------------------------------------------------
// Grammars
// ---------------------------------------------------------------------------

/// The grammar each language id is tokenised with: the two-face (bat) syntax of
/// that name.
///
/// The ids are Shiki's, because they are what [`language_for`] names and what a
/// fence tag is checked against. Shiki bundled a TextMate grammar for every one of
/// them; two-face carries Sublime syntaxes for most. The handful it has no grammar
/// for (see the test that lists them) render plain - the same thing the TypeScript
/// did for a grammar that failed to load.
///
/// A few map onto a near neighbour rather than going plain: JSON with comments and
/// JSON5 onto JSON (whose syntax already reads comments), MDX onto Markdown, Scheme
/// onto Lisp (whose syntax claims `.scm`), and both HCL ids onto Terraform.
const SYNTAXES: &[(&str, &str)] = &[
    ("actionscript-3", "ActionScript"),
    ("ada", "Ada"),
    ("apache", "Apache Conf"),
    ("applescript", "AppleScript"),
    ("asciidoc", "AsciiDoc (Asciidoctor)"),
    ("asm", "x86_64 Assembly"),
    ("awk", "AWK"),
    ("bat", "Batch File"),
    ("bibtex", "BibTeX"),
    ("c", "C"),
    ("clojure", "Clojure"),
    ("cmake", "CMake"),
    ("coffee", "CoffeeScript"),
    ("common-lisp", "Lisp"),
    ("cpp", "C++"),
    ("crystal", "Crystal"),
    ("csharp", "C#"),
    ("css", "CSS"),
    ("csv", "Separated Values"),
    ("d", "D"),
    ("dart", "Dart"),
    ("diff", "Diff"),
    ("docker", "Dockerfile"),
    ("dotenv", "DotENV"),
    ("elixir", "Elixir"),
    ("elm", "Elm"),
    ("erb", "HTML (Rails)"),
    ("erlang", "Erlang"),
    ("fish", "Fish"),
    ("fortran-fixed-form", "Fortran (Fixed Form)"),
    ("fortran-free-form", "Fortran (Modern)"),
    ("fsharp", "F#"),
    ("gdscript", "GDScript (Godot Engine)"),
    ("git-commit", "Git Commit"),
    ("git-rebase", "Git Rebase Todo"),
    ("glsl", "GLSL"),
    ("gnuplot", "gnuplot"),
    ("go", "Go"),
    ("graphql", "GraphQL"),
    ("groovy", "Groovy"),
    ("haml", "Ruby Haml"),
    ("haskell", "Haskell"),
    ("hcl", "Terraform"),
    ("html", "HTML"),
    ("http", "HTTP Request and Response"),
    ("ini", "INI"),
    ("java", "Java"),
    ("javascript", "JavaScript (Babel)"),
    ("jinja", "Jinja2"),
    ("json", "JSON"),
    ("json5", "JSON"),
    ("jsonc", "JSON"),
    ("jsonl", "JSON"),
    ("jsonnet", "jsonnet"),
    ("jsx", "JavaScript (Babel)"),
    ("julia", "Julia"),
    ("kotlin", "Kotlin"),
    ("latex", "LaTeX"),
    ("lean", "Lean 4"),
    ("less", "Less"),
    ("llvm", "LLVM"),
    ("log", "log"),
    ("lua", "Lua"),
    ("make", "Makefile"),
    ("markdown", "Markdown"),
    ("matlab", "MATLAB"),
    ("mdx", "Markdown"),
    ("nginx", "nginx"),
    ("nim", "Nim"),
    ("nix", "Nix"),
    ("nsis", "NSIS"),
    ("objective-c", "Objective-C"),
    ("objective-cpp", "Objective-C++"),
    ("ocaml", "OCaml"),
    ("odin", "Odin"),
    ("org", "orgmode"),
    ("pascal", "Pascal"),
    ("perl", "Perl"),
    ("php", "PHP"),
    ("powershell", "PowerShell"),
    ("proto", "Protocol Buffer"),
    ("puppet", "Puppet"),
    ("purescript", "PureScript"),
    ("python", "Python"),
    ("qml", "QML"),
    ("r", "R"),
    ("racket", "Racket"),
    ("regexp", "Regular Expression"),
    ("rst", "reStructuredText"),
    ("ruby", "Ruby"),
    ("rust", "Rust"),
    ("sass", "Sass"),
    ("scala", "Scala"),
    ("scheme", "Lisp"),
    ("scss", "SCSS"),
    ("shellscript", "Bourne Again Shell (bash)"),
    ("solidity", "Solidity"),
    ("sql", "SQL"),
    ("ssh-config", "SSH Config"),
    ("stylus", "Stylus"),
    ("svelte", "Svelte"),
    ("swift", "Swift"),
    ("system-verilog", "SystemVerilog"),
    ("tcl", "Tcl"),
    ("terraform", "Terraform"),
    ("tex", "TeX"),
    ("toml", "TOML"),
    ("tsv", "Tab Separated Values"),
    ("tsx", "TypeScriptReact"),
    ("twig", "HTML (Twig)"),
    ("typescript", "TypeScript"),
    ("typst", "Typst"),
    ("verilog", "Verilog"),
    ("vhdl", "VHDL"),
    ("viml", "VimL"),
    ("vue", "Vue Component"),
    ("vyper", "Vyper"),
    ("wgsl", "WGSL"),
    ("wikitext", "MediaWiki"),
    ("xml", "XML"),
    ("xsl", "XML"),
    ("yaml", "YAML"),
    ("zig", "Zig"),
];

/// Shiki's own aliases for the languages above, so a fence tag Shiki's registry
/// would have taken - ```ts, ```yml, ```c++ - resolves the same way here. Each maps
/// to the id it is an alias of.
const ALIASES: &[(&str, &str)] = &[
    ("actionscript", "actionscript-3"),
    ("adoc", "asciidoc"),
    ("as3", "actionscript-3"),
    ("bash", "shellscript"),
    ("batch", "bat"),
    ("c#", "csharp"),
    ("c++", "cpp"),
    ("cjs", "javascript"),
    ("clj", "clojure"),
    ("cmd", "bat"),
    ("coffeescript", "coffee"),
    ("cs", "csharp"),
    ("cts", "typescript"),
    ("dockerfile", "docker"),
    ("erl", "erlang"),
    ("f", "fortran-fixed-form"),
    ("f#", "fsharp"),
    ("f03", "fortran-free-form"),
    ("f08", "fortran-free-form"),
    ("f18", "fortran-free-form"),
    ("f77", "fortran-fixed-form"),
    ("f90", "fortran-free-form"),
    ("f95", "fortran-free-form"),
    ("for", "fortran-fixed-form"),
    ("fs", "fsharp"),
    ("gd", "gdscript"),
    ("gql", "graphql"),
    ("hs", "haskell"),
    ("jl", "julia"),
    ("js", "javascript"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("lean4", "lean"),
    ("lisp", "common-lisp"),
    ("makefile", "make"),
    ("md", "markdown"),
    ("mediawiki", "wikitext"),
    ("mjs", "javascript"),
    ("mts", "typescript"),
    ("objc", "objective-c"),
    ("properties", "ini"),
    ("protobuf", "proto"),
    ("ps", "powershell"),
    ("ps1", "powershell"),
    ("pwsh", "powershell"),
    ("py", "python"),
    ("rb", "ruby"),
    ("regex", "regexp"),
    ("rs", "rust"),
    ("sh", "shellscript"),
    ("shell", "shellscript"),
    ("styl", "stylus"),
    ("tf", "terraform"),
    ("tfvars", "terraform"),
    ("ts", "typescript"),
    ("typ", "typst"),
    ("vim", "viml"),
    ("vimscript", "viml"),
    ("vy", "vyper"),
    ("wiki", "wikitext"),
    ("yml", "yaml"),
    ("zsh", "shellscript"),
];

/// The id a language name stands for: itself when it is one, the id it is an
/// alias of when it is one of [`ALIASES`], and otherwise `None`.
fn known_language(name: &str) -> Option<&'static str> {
    SYNTAXES
        .iter()
        .find(|(id, _)| *id == name)
        .map(|(id, _)| *id)
        .or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| *alias == name)
                .map(|(_, id)| *id)
        })
}

/// The two-face syntax name a language is tokenised with, or `None` when there is
/// no grammar for it here.
pub fn syntax_name(language: &str) -> Option<&'static str> {
    let id = known_language(language)?;
    SYNTAXES
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, name)| *name)
}

/// Tags people write that are neither a language nor an extension, and would
/// otherwise read as prose.
const FENCE_ALIASES: &[(&str, &str)] = &[("golang", "go"), ("shell-session", "shellscript")];

/// The language a fence tag means, or `None` when nothing here knows.
///
/// The registry answers most of it, aliases included, so ```ts and ```yml resolve
/// without a table of our own. What it does not know is asked of the extension map,
/// which rescues the tags that are really file extensions - ```patch, ```conf - and
/// is the same map the diff resolves paths through. What is left is the short list
/// of [`FENCE_ALIASES`]: tags people write that are neither.
pub fn fence_language(tag: &str) -> Option<&'static str> {
    let lower = tag.to_lowercase();
    if let Some(id) = known_language(&lower) {
        return Some(id);
    }
    FENCE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, id)| *id)
        .or_else(|| language_for(&format!("fence.{lower}")))
}

// ---------------------------------------------------------------------------
// Themes
// ---------------------------------------------------------------------------

/// A VS Code colour theme, as the JSON carries it: the pieces this app reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxTheme {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    /// The workbench colours that are a single colour; the few that are a list
    /// (`symbolIcon.constantForeground` in the dark theme) are left out.
    #[serde(default, deserialize_with = "single_colours")]
    pub colors: HashMap<String, String>,
    #[serde(default)]
    pub token_colors: Vec<ThemeTokenRule>,
}

fn single_colours<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<String, String>, D::Error> {
    let all = HashMap::<String, serde_json::Value>::deserialize(deserializer)?;
    Ok(all
        .into_iter()
        .filter_map(|(key, value)| match value {
            serde_json::Value::String(colour) => Some((key, colour)),
            _ => None,
        })
        .collect())
}

/// The pair, chosen by measurement rather than taste: of fourteen matched light and
/// dark themes, this is the only one whose every token colour clears 4.5:1 against
/// the diff's own background - and 3:1 for the ones a theme mutes on purpose. It is
/// louder than the graphite palette around it was aiming for. That is the price of
/// the legibility test in `color` asserting something rather than describing
/// whatever the theme happened to do.
///
/// The JSON is Shiki's (`@shikijs/themes` 4.4.3, MIT), which is GitHub's
/// `github-vscode-theme` (MIT); see `data/themes/LICENSE`.
static LIGHT_THEME: LazyLock<SyntaxTheme> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/themes/github-light-default.json"))
        .unwrap_or_default()
});
static DARK_THEME: LazyLock<SyntaxTheme> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/themes/github-dark-default.json"))
        .unwrap_or_default()
});

/// The theme a mode highlights with: `github-dark-default` when dark, otherwise
/// `github-light-default`.
pub fn syntax_theme(dark: bool) -> &'static SyntaxTheme {
    if dark { &DARK_THEME } else { &LIGHT_THEME }
}

/// `FontStyle.NotSet` in vscode-textmate: a rule that says nothing about it.
const NOT_SET: i8 = -1;
const ITALIC: i8 = 1;
const BOLD: i8 = 2;
const UNDERLINE: i8 = 4;
const STRIKETHROUGH: i8 = 8;

/// A colour a theme rule may set, as `0xRRGGBBAA`; the hex notations
/// vscode-textmate's `isValidHexColor` accepts and nothing else.
fn theme_colour(value: Option<&str>) -> Option<u32> {
    let value = value?;
    let digits = value.strip_prefix('#')?;
    if !matches!(digits.len(), 3 | 4 | 6 | 8) {
        return None;
    }
    parse_color(value).ok().map(|color| color.to_u32())
}

/// One rule of the trie: what it sets, how deep in the trie it was set, and the
/// ancestors it requires (nearest first, `>` for "the very next one").
#[derive(Debug, Clone)]
struct TrieRule {
    scope_depth: usize,
    parent_scopes: Vec<String>,
    font_style: i8,
    foreground: Option<u32>,
    background: Option<u32>,
}

impl TrieRule {
    fn accept_overwrite(
        &mut self,
        scope_depth: usize,
        font_style: i8,
        foreground: Option<u32>,
        background: Option<u32>,
    ) {
        self.scope_depth = self.scope_depth.max(scope_depth);
        if font_style != NOT_SET {
            self.font_style = font_style;
        }
        if foreground.is_some() {
            self.foreground = foreground;
        }
        if background.is_some() {
            self.background = background;
        }
    }
}

/// vscode-textmate's `ThemeTrieElement`: theme rules keyed by the dot-separated
/// segments of their scope, each node inheriting what its parent sets.
#[derive(Debug, Clone)]
struct TrieNode {
    main: TrieRule,
    with_parents: Vec<TrieRule>,
    children: HashMap<String, TrieNode>,
}

impl TrieNode {
    fn insert(
        &mut self,
        scope_depth: usize,
        scope: &str,
        parent_scopes: Option<&[String]>,
        font_style: i8,
        foreground: Option<u32>,
        background: Option<u32>,
    ) {
        if scope.is_empty() {
            self.insert_here(
                scope_depth,
                parent_scopes,
                font_style,
                foreground,
                background,
            );
            return;
        }
        let (head, tail) = scope.split_once('.').unwrap_or((scope, ""));
        let main = &self.main;
        let with_parents = &self.with_parents;
        let child = self
            .children
            .entry(head.to_string())
            .or_insert_with(|| TrieNode {
                main: main.clone(),
                with_parents: with_parents.clone(),
                children: HashMap::new(),
            });
        child.insert(
            scope_depth + 1,
            tail,
            parent_scopes,
            font_style,
            foreground,
            background,
        );
    }

    fn insert_here(
        &mut self,
        scope_depth: usize,
        parent_scopes: Option<&[String]>,
        mut font_style: i8,
        mut foreground: Option<u32>,
        mut background: Option<u32>,
    ) {
        let Some(parent_scopes) = parent_scopes else {
            self.main
                .accept_overwrite(scope_depth, font_style, foreground, background);
            return;
        };
        if let Some(rule) = self
            .with_parents
            .iter_mut()
            .find(|rule| rule.parent_scopes == parent_scopes)
        {
            rule.accept_overwrite(scope_depth, font_style, foreground, background);
            return;
        }
        // Inherit from the main rule what this one leaves unset.
        if font_style == NOT_SET {
            font_style = self.main.font_style;
        }
        if foreground.is_none() {
            foreground = self.main.foreground;
        }
        if background.is_none() {
            background = self.main.background;
        }
        self.with_parents.push(TrieRule {
            scope_depth,
            parent_scopes: parent_scopes.to_vec(),
            font_style,
            foreground,
            background,
        });
    }

    /// The candidate rules for one scope name, most specific first.
    fn matches(&self, scope: &str) -> Vec<&TrieRule> {
        if !scope.is_empty() {
            let (head, tail) = scope.split_once('.').unwrap_or((scope, ""));
            if let Some(child) = self.children.get(head) {
                return child.matches(tail);
            }
        }
        let mut rules: Vec<&TrieRule> = self.with_parents.iter().chain([&self.main]).collect();
        rules.sort_by(|a, b| specificity(a, b));
        rules
    }
}

/// vscode-textmate's `_cmpBySpecificity`: deeper first, then longer parent scopes,
/// then more of them.
fn specificity(a: &TrieRule, b: &TrieRule) -> std::cmp::Ordering {
    if a.scope_depth != b.scope_depth {
        return b.scope_depth.cmp(&a.scope_depth);
    }
    let (mut ai, mut bi) = (0, 0);
    loop {
        if a.parent_scopes.get(ai).is_some_and(|scope| scope == ">") {
            ai += 1;
        }
        if b.parent_scopes.get(bi).is_some_and(|scope| scope == ">") {
            bi += 1;
        }
        let (Some(a_scope), Some(b_scope)) = (a.parent_scopes.get(ai), b.parent_scopes.get(bi))
        else {
            break;
        };
        // Lengths in UTF-16 units, as JavaScript counts them.
        let a_len = a_scope.encode_utf16().count();
        let b_len = b_scope.encode_utf16().count();
        if a_len != b_len {
            return b_len.cmp(&a_len);
        }
        ai += 1;
        bi += 1;
    }
    b.parent_scopes.len().cmp(&a.parent_scopes.len())
}

/// vscode-textmate's `_scopePathMatchesParentScopes`: every parent selector matches
/// an ancestor, in order, outwards. `ancestors` is nearest first.
fn parents_match(ancestors: &[&str], parent_scopes: &[String]) -> bool {
    let mut path = ancestors.iter();
    let mut index = 0;
    while index < parent_scopes.len() {
        let mut pattern = parent_scopes[index].as_str();
        let mut must_match = false;
        if pattern == ">" {
            if index == parent_scopes.len() - 1 {
                return false;
            }
            index += 1;
            pattern = parent_scopes[index].as_str();
            must_match = true;
        }
        loop {
            let Some(scope) = path.next() else {
                return false;
            };
            if selector_matches(pattern, scope) {
                break;
            }
            if must_match {
                return false;
            }
        }
        index += 1;
    }
    true
}

/// What a scope resolves to: vscode-textmate's encoded font style, foreground and
/// background, with `None` meaning "inherit".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Attributes {
    font_style: i8,
    foreground: u32,
    background: u32,
}

/// A theme compiled the way vscode-textmate (and so Shiki) compiles one, so a token
/// takes the colour it took in the TypeScript app given the same scopes.
///
/// syntect has its own theme resolution, but it scores selectors differently, and
/// the point of carrying these exact themes over is carrying their exact colours.
/// The algorithm is small: a trie of rules by scope segment, matched against each
/// scope as it is pushed, the most specific rule whose parent selectors match the
/// enclosing scopes winning, and whatever it leaves unset inherited from the scope
/// it was pushed onto.
#[derive(Debug)]
struct CompiledTheme {
    defaults: Attributes,
    root: TrieNode,
    background_rules: Vec<BackgroundRule>,
    /// [`CompiledTheme::background_rules`]' colours, parsed once.
    background_colours: Vec<Option<u32>>,
}

impl CompiledTheme {
    fn new(theme: &SyntaxTheme) -> CompiledTheme {
        struct Parsed {
            scope: String,
            parent_scopes: Option<Vec<String>>,
            index: usize,
            font_style: i8,
            foreground: Option<u32>,
            background: Option<u32>,
        }

        let mut parsed: Vec<Parsed> = Vec::new();
        for (index, rule) in theme.token_colors.iter().enumerate() {
            let Some(settings) = &rule.settings else {
                continue;
            };
            let scopes: Vec<String> = match &rule.scope {
                Some(ThemeScope::One(scope)) => scope
                    .trim_start_matches(',')
                    .trim_end_matches(',')
                    .split(',')
                    .map(str::to_string)
                    .collect(),
                Some(ThemeScope::Many(scopes)) => scopes.clone(),
                None => vec![String::new()],
            };
            let font_style = match &settings.font_style {
                None => NOT_SET,
                Some(style) => style.split(' ').fold(0, |flags, segment| {
                    flags
                        | match segment {
                            "italic" => ITALIC,
                            "bold" => BOLD,
                            "underline" => UNDERLINE,
                            "strikethrough" => STRIKETHROUGH,
                            _ => 0,
                        }
                }),
            };
            let foreground = theme_colour(settings.foreground.as_deref());
            let background = theme_colour(settings.background.as_deref());
            for scope in scopes {
                let segments: Vec<&str> = scope.trim().split(' ').collect();
                let (last, parents) = segments.split_last().unwrap_or((&"", &[]));
                parsed.push(Parsed {
                    scope: last.to_string(),
                    parent_scopes: (!parents.is_empty())
                        .then(|| parents.iter().rev().map(|s| s.to_string()).collect()),
                    index,
                    font_style,
                    foreground,
                    background,
                });
            }
        }

        // Sorted by scope, so a node is always inserted before the nodes under it
        // and they inherit from it; then by parent scopes, then by order.
        parsed.sort_by(|a, b| {
            a.scope
                .cmp(&b.scope)
                .then_with(|| match (&a.parent_scopes, &b.parent_scopes) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(a), Some(b)) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                })
                .then_with(|| a.index.cmp(&b.index))
        });

        // Shiki puts the editor's own colours in front as the scope-less rule, which
        // is what every token without a rule of its own is painted with.
        let mut defaults = Attributes {
            font_style: 0,
            foreground: theme_colour(theme.colors.get("editor.foreground").map(String::as_str))
                .unwrap_or(0x0000_00ff),
            background: theme_colour(theme.colors.get("editor.background").map(String::as_str))
                .unwrap_or(0xffff_ffff),
        };
        let mut rest = parsed.as_slice();
        while let Some((first, tail)) = rest.split_first() {
            if !first.scope.is_empty() {
                break;
            }
            if first.font_style != NOT_SET {
                defaults.font_style = first.font_style;
            }
            if let Some(foreground) = first.foreground {
                defaults.foreground = foreground;
            }
            if let Some(background) = first.background {
                defaults.background = background;
            }
            rest = tail;
        }

        let mut root = TrieNode {
            main: TrieRule {
                scope_depth: 0,
                parent_scopes: Vec::new(),
                font_style: NOT_SET,
                foreground: None,
                background: None,
            },
            with_parents: Vec::new(),
            children: HashMap::new(),
        };
        for rule in rest {
            root.insert(
                0,
                &rule.scope,
                rule.parent_scopes.as_deref(),
                rule.font_style,
                rule.foreground,
                rule.background,
            );
        }

        let background_rules = background_rules(&theme.token_colors);
        CompiledTheme {
            defaults,
            root,
            background_colours: background_rules
                .iter()
                .map(|rule| theme_colour(Some(rule.background.as_str())))
                .collect(),
            background_rules,
        }
    }
}

// ---------------------------------------------------------------------------
// The highlighter
// ---------------------------------------------------------------------------

/// How a token's text is set, beyond its colour.
///
/// The TypeScript renderer carried only the colours through to the page (its
/// `.tok` rule reads `--shiki-light` and `--shiki-dark` and nothing else), so a
/// view after parity with it ignores these; they are here because the themes do
/// set them, on headings, emphasis and invalid code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FontStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl FontStyle {
    fn from_flags(flags: i8) -> FontStyle {
        if flags <= 0 {
            return FontStyle::default();
        }
        FontStyle {
            bold: flags & BOLD != 0,
            italic: flags & ITALIC != 0,
            underline: flags & UNDERLINE != 0,
            strikethrough: flags & STRIKETHROUGH != 0,
        }
    }
}

/// One run of characters sharing a colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Byte range of the run within its line (the line without its newline). Always
    /// on character boundaries.
    pub range: Range<usize>,
    /// `0xRRGGBBAA`, as `gpui::rgba` takes it.
    pub color: u32,
    pub font_style: FontStyle,
    /// The theme's own background, for the few tokens a theme paints one behind;
    /// `None` leaves the row showing through. Only ever set for the languages
    /// [`paints_backgrounds`] names.
    pub background: Option<u32>,
}

impl Token {
    /// The run's text, given the line it was produced from.
    pub fn text<'a>(&self, line: &'a str) -> &'a str {
        line.get(self.range.clone()).unwrap_or("")
    }
}

/// Lines longer than this are not tokenized and render in the theme's plain
/// foreground.
///
/// Shiki gave each line a time budget (500ms by default) and left the rest of a line
/// that ran over it plain; a length cap is the deterministic version of the same
/// guard. Minified bundles and lock-file blobs are what reach it, and their colours
/// were never worth the stall.
pub const MAX_LINE_BYTES: usize = 20_000;

struct Loaded {
    syntaxes: SyntaxSet,
    light: CompiledTheme,
    dark: CompiledTheme,
}

/// The syntax highlighter: two-face's grammars and the two themes, loaded once on
/// first use.
///
/// Constructing one costs nothing; the grammars and themes are loaded the first
/// time anything is highlighted (or on [`Highlighter::preload`]), and every grammar
/// is compiled the first time a file of its language comes up, then kept. It is
/// `Send + Sync`, so the UI can highlight on a background thread and share one
/// instance ([`Highlighter::shared`]) across all of them.
///
/// Every call blocks until it is done, so it belongs off the UI thread. A text of
/// more than a few hundred lines is tokenized on several short-lived threads at
/// once (see `tokenize_lines`), with the same result as on one.
pub struct Highlighter {
    loaded: OnceLock<Loaded>,
}

impl Default for Highlighter {
    fn default() -> Self {
        Highlighter::new()
    }
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter")
            .field("loaded", &self.loaded.get().is_some())
            .finish()
    }
}

static SHARED: Highlighter = Highlighter::new();

impl Highlighter {
    pub const fn new() -> Highlighter {
        Highlighter {
            loaded: OnceLock::new(),
        }
    }

    /// The one highlighter the app shares: a snippet in a description and the code
    /// it is about are coloured by one thing, and a grammar compiled for either
    /// serves both.
    pub fn shared() -> &'static Highlighter {
        &SHARED
    }

    /// Loads the grammars and themes now rather than on first use.
    pub fn preload(&self) {
        self.loaded();
    }

    fn loaded(&self) -> &Loaded {
        self.loaded.get_or_init(|| Loaded {
            syntaxes: two_face::syntax::extra_newlines(),
            light: CompiledTheme::new(syntax_theme(false)),
            dark: CompiledTheme::new(syntax_theme(true)),
        })
    }

    /// Whether there is a grammar for a language (an id or one of its aliases).
    pub fn supports(&self, language: &str) -> bool {
        syntax_name(language).is_some()
    }

    fn syntax(&self, language: &str) -> Option<(&Loaded, &SyntaxReference)> {
        let name = syntax_name(language)?;
        let loaded = self.loaded();
        let syntax = loaded.syntaxes.find_syntax_by_name(name)?;
        Some((loaded, syntax))
    }

    /// Tokens for every line of `text` (split on `\n`, one token line per source
    /// line), in the light or the dark theme.
    ///
    /// Empty when there is no grammar for the language or the text trips the
    /// parser up: that should cost the reader the colour and nothing else.
    pub fn highlight_lines(&self, language: &str, text: &str, dark: bool) -> Vec<Vec<Token>> {
        let Some((loaded, syntax)) = self.syntax(language) else {
            return Vec::new();
        };
        tokenize(loaded, syntax, language, text, dark).unwrap_or_default()
    }

    /// Tokens for every line of every hunk: one entry per hunk, holding one slot per
    /// line of it (see [`hunk_tokens`]).
    ///
    /// Every slot is `None` rather than anything failing when the language has no
    /// grammar or a side trips the parser up.
    pub fn highlight_hunks(
        &self,
        hunks: &[DiffHunk],
        language: &str,
        dark: bool,
    ) -> Vec<Vec<Option<Vec<Token>>>> {
        let plain = || {
            hunks
                .iter()
                .map(|hunk| hunk.lines.iter().map(|_| None).collect())
                .collect()
        };
        let Some((loaded, syntax)) = self.syntax(language) else {
            return plain();
        };

        let side = |hunk: &DiffHunk, side: DiffSide| -> Option<Vec<Vec<Token>>> {
            let text = side_text(hunk, side);
            if text.is_empty() {
                return Some(Vec::new());
            }
            tokenize(loaded, syntax, language, &text, dark)
        };

        let mut highlighted = Vec::with_capacity(hunks.len());
        for hunk in hunks {
            let (Some(old_side), Some(new_side)) = (side(hunk, Side::Old), side(hunk, Side::New))
            else {
                return plain();
            };
            highlighted.push(hunk_tokens(hunk, old_side, new_side));
        }
        highlighted
    }

    /// Tokens for a fenced code block, or `None` when the fence carries no language
    /// this build can resolve - in which case it stays the preformatted text it
    /// already was.
    ///
    /// The same highlighter, themes and grammar cache as the diff: a snippet in a
    /// description and the code it is about are coloured by one thing.
    pub fn highlight_code(&self, code: &str, tag: &str, dark: bool) -> Option<Vec<Vec<Token>>> {
        let language = fence_language(tag)?;
        let (loaded, syntax) = self.syntax(language)?;
        tokenize(loaded, syntax, language, code, dark)
    }
}

/// [`Highlighter::highlight_lines`] on the shared highlighter.
pub fn highlight_lines(language: &str, text: &str, dark: bool) -> Vec<Vec<Token>> {
    Highlighter::shared().highlight_lines(language, text, dark)
}

/// [`Highlighter::highlight_code`] on the shared highlighter.
pub fn highlight_code(code: &str, tag: &str, dark: bool) -> Option<Vec<Vec<Token>>> {
    Highlighter::shared().highlight_code(code, tag, dark)
}

/// [`Highlighter::highlight_hunks`] on the shared highlighter.
pub fn highlight_hunks(
    hunks: &[DiffHunk],
    language: &str,
    dark: bool,
) -> Vec<Vec<Option<Vec<Token>>>> {
    Highlighter::shared().highlight_hunks(hunks, language, dark)
}

/// What one scope resolves against a theme, computed once per scope per call.
struct ScopeInfo<'t> {
    name: String,
    /// The trie's candidates for this scope name, most specific first.
    rules: Vec<&'t TrieRule>,
    /// The background rule this scope alone would pick.
    background: Option<usize>,
}

/// One level of the scope stack, resolved.
struct Level<'t> {
    scope: Rc<ScopeInfo<'t>>,
    attributes: Attributes,
    background: Option<usize>,
}

/// Resolves styles as the parser pushes and pops scopes - vscode-textmate's
/// `AttributedScopeStack`.
struct Painter<'t> {
    theme: &'t CompiledTheme,
    backgrounds: bool,
    scopes: HashMap<Scope, Rc<ScopeInfo<'t>>>,
    stack: Vec<Level<'t>>,
}

impl<'t> Painter<'t> {
    fn new(theme: &'t CompiledTheme, backgrounds: bool) -> Painter<'t> {
        Painter {
            theme,
            backgrounds,
            scopes: HashMap::new(),
            stack: Vec::new(),
        }
    }

    fn info(&mut self, scope: Scope) -> Rc<ScopeInfo<'t>> {
        let theme = self.theme;
        let backgrounds = self.backgrounds;
        self.scopes
            .entry(scope)
            .or_insert_with(|| {
                let name = scope.build_string();
                Rc::new(ScopeInfo {
                    rules: theme.root.matches(&name),
                    background: if backgrounds {
                        best_background_rule(&theme.background_rules, &name)
                    } else {
                        None
                    },
                    name,
                })
            })
            .clone()
    }

    fn top(&self) -> (Attributes, Option<usize>) {
        self.stack
            .last()
            .map_or((self.theme.defaults, None), |level| {
                (level.attributes, level.background)
            })
    }

    fn push(&mut self, scope: Scope) {
        let info = self.info(scope);
        let (mut attributes, background) = self.top();

        // The enclosing scopes, nearest first - gathered only for a candidate that
        // has parent selectors to check, which most never do.
        let mut ancestors: Option<Vec<&str>> = None;
        let stack = &self.stack;
        let effective = info.rules.iter().find(|rule| {
            if rule.parent_scopes.is_empty() {
                return true;
            }
            let ancestors = ancestors.get_or_insert_with(|| {
                stack
                    .iter()
                    .rev()
                    .map(|level| level.scope.name.as_str())
                    .collect()
            });
            parents_match(ancestors, &rule.parent_scopes)
        });
        if let Some(rule) = effective {
            if rule.font_style != NOT_SET {
                attributes.font_style = rule.font_style;
            }
            if let Some(foreground) = rule.foreground {
                attributes.foreground = foreground;
            }
            if let Some(background) = rule.background {
                attributes.background = background;
            }
        }

        let background =
            better_background_rule(&self.theme.background_rules, background, info.background);
        self.stack.push(Level {
            scope: info,
            attributes,
            background,
        });
    }

    fn pop(&mut self) {
        self.stack.pop();
    }
}

/// Collects one line's runs, merging neighbours that look the same as
/// vscode-textmate merges tokens with equal metadata.
struct LineTokens {
    tokens: Vec<Token>,
    /// The theme-resolved background of the last token, part of what "looks the
    /// same" means.
    last_theme_background: u32,
}

impl LineTokens {
    fn new() -> LineTokens {
        LineTokens {
            tokens: Vec::new(),
            last_theme_background: 0,
        }
    }

    fn push(&mut self, range: Range<usize>, attributes: Attributes, background: Option<u32>) {
        let font_style = FontStyle::from_flags(attributes.font_style);
        if let Some(last) = self.tokens.last_mut()
            && last.range.end == range.start
            && last.color == attributes.foreground
            && last.font_style == font_style
            && self.last_theme_background == attributes.background
        {
            last.range.end = range.end;
            // A token spanning several scope ranges is painted only where every
            // range agrees on the background, and a disagreement leaves the row
            // showing through rather than guessing which half was right. Once
            // `None`, a merged token stays `None`: any later run either agrees
            // with that or disagrees with it.
            if last.background != background {
                last.background = None;
            }
            return;
        }
        self.tokens.push(Token {
            range,
            color: attributes.foreground,
            font_style,
            background,
        });
        self.last_theme_background = attributes.background;
    }
}

/// Below this many lines a text is tokenized on the calling thread alone; above it,
/// in [lanes](tokenize_lines) on several.
const PARALLEL_MIN_LINES: usize = 600;

/// How many lines a lane is given. Small enough that the lanes even out between
/// fast and slow cores, which pull them from a shared queue; big enough that a
/// lane's run past its end to the next top-level line stays a small fraction of it.
const LINES_PER_LANE: usize = 150;

/// The most lanes one text is split into.
const MAX_LANES: usize = 64;

/// The most threads one text is tokenized on.
const MAX_THREADS: usize = 16;

/// Tokenizes `text` line by line (split on `\n`); `None` when the parser fails.
fn tokenize(
    loaded: &Loaded,
    syntax: &SyntaxReference,
    language: &str,
    text: &str,
    dark: bool,
) -> Option<Vec<Vec<Token>>> {
    let lines: Vec<&str> = text.split('\n').collect();
    let lanes = if lines.len() < PARALLEL_MIN_LINES {
        1
    } else {
        (lines.len() / LINES_PER_LANE).clamp(1, MAX_LANES)
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |cores| cores.get())
        .min(MAX_THREADS);
    let context = Context {
        loaded,
        syntax,
        theme: if dark { &loaded.dark } else { &loaded.light },
        backgrounds: paints_backgrounds(known_language(language).unwrap_or(language)),
    };
    tokenize_lines(&context, &lines, lanes, threads)
}

/// Everything a lane needs to tokenize, shared by all of them.
#[derive(Clone, Copy)]
struct Context<'a> {
    loaded: &'a Loaded,
    syntax: &'a SyntaxReference,
    theme: &'a CompiledTheme,
    backgrounds: bool,
}

/// One parser working through lines in order, with the scope stack and the styles
/// it resolves to.
struct LaneParser<'a> {
    context: Context<'a>,
    painter: Painter<'a>,
    state: ParseState,
    stack: ScopeStack,
    /// The parser's and the scope stack's state between two top-level lines of the
    /// grammar - what parsing a blank first line leaves behind.
    root: (ParseState, ScopeStack),
    buffer: String,
}

impl<'a> LaneParser<'a> {
    /// A parser at the very start of a text (`at_root` false), or one that starts
    /// in the state between two top-level lines (`at_root` true).
    fn new(context: Context<'a>, at_root: bool) -> Option<LaneParser<'a>> {
        let mut root_state = ParseState::new(context.syntax);
        let mut root_stack = ScopeStack::new();
        for (_, op) in root_state.parse_line("\n", &context.loaded.syntaxes).ok()? {
            root_stack.apply(&op).ok()?;
        }

        let mut painter = Painter::new(context.theme, context.backgrounds);
        let (state, stack) = if at_root {
            for scope in root_stack.as_slice() {
                painter.push(*scope);
            }
            (root_state.clone(), root_stack.clone())
        } else {
            (ParseState::new(context.syntax), ScopeStack::new())
        };

        Some(LaneParser {
            context,
            painter,
            state,
            stack,
            root: (root_state, root_stack),
            buffer: String::new(),
        })
    }

    /// Whether the parser is in the root state right now. Two parsers that both are
    /// tokenize everything after this point identically, which is what lets a lane
    /// that started from a guess take over from one that knows.
    fn at_root(&self) -> bool {
        self.state == self.root.0 && self.stack == self.root.1
    }

    /// The tokens of one line, advancing the parser past it.
    fn line(&mut self, raw: &str) -> Option<Vec<Token>> {
        // Shiki splits on `\r?\n`, so a carriage return ending a line is part of the
        // break, not of the text.
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let theme = self.context.theme;
        let mut tokens = LineTokens::new();

        if line.len() > MAX_LINE_BYTES {
            let (attributes, _) = self.painter.top();
            tokens.push(
                0..line.len(),
                Attributes {
                    foreground: theme.defaults.foreground,
                    font_style: theme.defaults.font_style,
                    ..attributes
                },
                None,
            );
            return Some(tokens.tokens);
        }

        self.buffer.clear();
        self.buffer.push_str(line);
        self.buffer.push('\n');
        let ops = self
            .state
            .parse_line(&self.buffer, &self.context.loaded.syntaxes)
            .ok()?;

        let emit = |tokens: &mut LineTokens, painter: &Painter, range: Range<usize>| {
            let (attributes, background) = painter.top();
            let background =
                background.and_then(|index| theme.background_colours.get(index).copied().flatten());
            tokens.push(range, attributes, background);
        };

        let mut pos = 0;
        for (index, op) in &ops {
            let end = (*index).min(line.len());
            if end > pos {
                emit(&mut tokens, &self.painter, pos..end);
                pos = end;
            }
            let painter = &mut self.painter;
            self.stack
                .apply_with_hook(op, |basic, _| match basic {
                    BasicScopeStackOp::Push(scope) => painter.push(scope),
                    BasicScopeStackOp::Pop => painter.pop(),
                })
                .ok()?;
        }
        if line.len() > pos {
            emit(&mut tokens, &self.painter, pos..line.len());
        }
        Some(tokens.tokens)
    }
}

/// What one lane produced: the tokens of the lines `first..first + tokens.len()`,
/// and for each of them whether the lane's parser was in the root state just
/// before it.
struct Lane {
    first: usize,
    tokens: Vec<Vec<Token>>,
    at_root: Vec<bool>,
}

impl Lane {
    fn end(&self) -> usize {
        self.first + self.tokens.len()
    }

    /// Whether this lane can be trusted from `line` on, given that the true parse is
    /// in the root state there.
    fn takes_over_at(&self, line: usize) -> bool {
        line >= self.first
            && line < self.end()
            && self.at_root.get(line - self.first).copied() == Some(true)
    }
}

/// Parses `lines[first..]` until it reaches a line at or past `stop_after` in the
/// root state (which it leaves to the next lane), or the end, or - when given - a
/// line some lane in `others` can take over at.
fn run_lane(
    context: Context<'_>,
    lines: &[&str],
    first: usize,
    at_root: bool,
    stop_after: usize,
    others: &[Lane],
) -> Option<Lane> {
    let mut parser = LaneParser::new(context, at_root)?;
    let mut lane = Lane {
        first,
        tokens: Vec::new(),
        at_root: Vec::new(),
    };
    for (line, text) in lines.iter().enumerate().skip(first) {
        let rooted = parser.at_root();
        if rooted
            && line > first
            && (line >= stop_after || others.iter().any(|other| other.takes_over_at(line)))
        {
            break;
        }
        lane.at_root.push(rooted);
        lane.tokens.push(parser.line(text)?);
    }
    Some(lane)
}

/// Tokenizes lines in `lanes` lanes on up to `threads` threads, with exactly the
/// result one parser going through them in order would give.
///
/// A grammar's parser carries state from line to line, so a text cannot simply be
/// cut up - except where that state is the one between two top-level lines, which
/// in most source files it is every few lines. So each lane but the first starts
/// at its share of the text *from* that root state, as a guess, and runs until it
/// is in the root state again past the start of the next lane. Then the lanes are
/// stitched together in order: the first is right by construction, and wherever
/// the lane being trusted stops (in the root state, so the truth is in the root
/// state there) a later lane that was in the root state at that same line produced
/// exactly what the truth would have from then on, and takes over. Where no lane
/// can, the rest is parsed in order on this thread until one can.
///
/// A text that never returns to the root state - one giant class, say - degrades
/// to parsing in order; nothing is ever taken from a lane that was not provably in
/// step with the true parse.
fn tokenize_lines(
    context: &Context<'_>,
    lines: &[&str],
    lanes: usize,
    threads: usize,
) -> Option<Vec<Vec<Token>>> {
    tokenize_in_lanes(context, lines, lanes, threads).map(|(tokens, _)| tokens)
}

/// [`tokenize_lines`], also saying how many of the lines came from a lane that
/// started from a guess - which is how the tests know the lanes did anything.
fn tokenize_in_lanes(
    context: &Context<'_>,
    lines: &[&str],
    lanes: usize,
    threads: usize,
) -> Option<(Vec<Vec<Token>>, usize)> {
    let n = lines.len();
    if lanes <= 1 || threads <= 1 || n < 2 {
        return run_lane(*context, lines, 0, false, n, &[]).map(|lane| (lane.tokens, 0));
    }

    let starts: Vec<usize> = (0..lanes).map(|k| k * n / lanes).collect();
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<Lane>> = (0..lanes).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads.min(lanes))
            .map(|_| {
                let (starts, next, context) = (&starts, &next, *context);
                scope.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&first) = starts.get(k) else {
                            break;
                        };
                        let stop_after = starts.get(k + 1).copied().unwrap_or(n);
                        done.push((k, run_lane(context, lines, first, k > 0, stop_after, &[])));
                    }
                    done
                })
            })
            .collect();
        for handle in handles {
            // A thread that panicked leaves its lanes missing, which is handled
            // below like a lane that failed.
            if let Ok(done) = handle.join() {
                for (k, lane) in done {
                    results[k] = lane;
                }
            }
        }
    });

    // A lane whose guess tripped the parser up is simply not available; the first
    // lane failing is the text failing.
    let mut results = results.into_iter();
    let first = results.next().flatten()?;
    let mut others: Vec<Lane> = results.flatten().collect();

    let mut out: Vec<Vec<Token>> = Vec::with_capacity(n);
    let mut guessed = 0;
    let mut trusted = first;
    let mut trusted_guessed = false;
    let mut from = 0;
    loop {
        let line = trusted.end();
        if trusted_guessed {
            guessed += line - from;
        }
        out.extend(trusted.tokens.drain(from - trusted.first..));
        if line >= n {
            break;
        }
        // The trusted lane stopped in the root state, so the truth is in it here.
        let taking = others.iter().position(|lane| lane.takes_over_at(line));
        trusted_guessed = taking.is_some();
        trusted = match taking {
            Some(index) => {
                // Taken whole: the lines it covers are behind the stitch from here on,
                // so emptying it also stops it being offered again.
                let lane = &mut others[index];
                Lane {
                    first: lane.first,
                    tokens: std::mem::take(&mut lane.tokens),
                    at_root: Vec::new(),
                }
            }
            None => run_lane(*context, lines, line, true, n, &others)?,
        };
        from = line;
    }
    Some((out, guessed))
}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Highlighter>();
    send_sync::<Token>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{parse_patch, to_split_rows};

    /// One token line per source line, standing in for whatever the highlighter emits.
    fn token_lines(text: &str) -> Vec<Vec<String>> {
        text.split('\n')
            .map(|line| vec![line.to_string()])
            .collect()
    }

    fn one(patch: &[&str]) -> DiffHunk {
        parse_patch(&patch.join("\n"))
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no hunk in {patch:?}"))
    }

    fn zipped(hunk: &DiffHunk) -> Vec<Option<Vec<String>>> {
        hunk_tokens(
            hunk,
            token_lines(&side_text(hunk, Side::Old)),
            token_lines(&side_text(hunk, Side::New)),
        )
    }

    fn strings(values: &[&str]) -> Option<Vec<String>> {
        Some(values.iter().map(|value| value.to_string()).collect())
    }

    // --- test/highlight.test.ts ---

    #[test]
    fn side_text_reconstructs_each_side_from_the_lines_that_belong_to_it() {
        let hunk = one(&[
            "@@ -1,4 +1,4 @@",
            " const a = 1",
            "-const b = 2",
            "+const b = 3",
            " const c = 4",
        ]);
        assert_eq!(
            side_text(&hunk, Side::Old),
            "const a = 1\nconst b = 2\nconst c = 4"
        );
        assert_eq!(
            side_text(&hunk, Side::New),
            "const a = 1\nconst b = 3\nconst c = 4"
        );
    }

    #[test]
    fn side_text_leaves_meta_lines_out() {
        let hunk = one(&[
            "@@ -1,2 +1,2 @@",
            "-a",
            "\\ No newline at end of file",
            "+b",
            " c",
        ]);
        assert_eq!(side_text(&hunk, Side::Old), "a\nc");
        assert_eq!(side_text(&hunk, Side::New), "b\nc");
        assert_eq!(side_lines(&hunk, Side::Old).len(), 2);
    }

    #[test]
    fn side_text_keeps_a_blank_context_line_so_the_lines_after_it_stay_aligned() {
        let hunk = one(&["@@ -1,4 +1,4 @@", " a", "", " b", "-c", "+d"]);
        assert_eq!(side_text(&hunk, Side::Old), "a\n\nb\nc");
        assert_eq!(side_text(&hunk, Side::New), "a\n\nb\nd");
    }

    #[test]
    fn hunk_tokens_zips_an_interleaved_replacement_run_onto_the_right_lines() {
        let hunk = one(&[
            "@@ -1,5 +1,5 @@",
            " keep one",
            "-old two",
            "-old three",
            "+new two",
            "+new three",
            " keep four",
        ]);
        let tokens = zipped(&hunk);
        // Every line gets the tokens of its own text, not of the line beside it.
        for (index, line) in hunk.lines.iter().enumerate() {
            assert_eq!(
                tokens[index],
                Some(vec![line.content.clone()]),
                "mismatched on {}",
                line.content
            );
        }
    }

    #[test]
    fn hunk_tokens_handles_a_hunk_that_is_nothing_but_additions() {
        let hunk = one(&["@@ -0,0 +1,3 @@", "+one", "+two", "+three"]);
        assert_eq!(side_text(&hunk, Side::Old), "");
        // The TypeScript's highlighter is never asked about an empty side.
        let tokens = hunk_tokens(&hunk, Vec::new(), token_lines(&side_text(&hunk, Side::New)));
        assert_eq!(
            tokens,
            [strings(&["one"]), strings(&["two"]), strings(&["three"])]
        );
        // And handing it the one empty line `split` makes of "" changes nothing.
        assert_eq!(zipped(&hunk), tokens);
    }

    #[test]
    fn hunk_tokens_handles_a_hunk_that_is_nothing_but_deletions() {
        let hunk = one(&["@@ -1,3 +0,0 @@", "-one", "-two", "-three"]);
        assert_eq!(side_text(&hunk, Side::New), "");
        assert_eq!(
            zipped(&hunk),
            [strings(&["one"]), strings(&["two"]), strings(&["three"])]
        );
    }

    #[test]
    fn hunk_tokens_leaves_a_line_the_highlighter_said_nothing_about_unmapped() {
        let hunk = one(&["@@ -1,2 +1,2 @@", " a", "-b", "+c"]);
        // Two lines on the old side, one token line: the second stays plain rather
        // than borrowing tokens that belong to another line.
        let tokens = hunk_tokens(
            &hunk,
            vec![vec!["a".to_string()]],
            token_lines(&side_text(&hunk, Side::New)),
        );
        assert_eq!(tokens[0], strings(&["a"]));
        assert_eq!(tokens[1], None);
    }

    #[test]
    fn hunk_tokens_lands_on_the_right_cell_of_every_split_row() {
        let hunk = one(&["@@ -1,4 +1,4 @@", " shared", "-removed", "+added", " tail"]);
        let tokens = zipped(&hunk);
        let cells: Vec<_> = to_split_rows(&hunk)
            .into_iter()
            .map(|row| {
                (
                    row.left.and_then(|i| tokens[i].clone()),
                    row.right.and_then(|i| tokens[i].clone()),
                )
            })
            .collect();
        assert_eq!(
            cells,
            [
                (strings(&["shared"]), strings(&["shared"])),
                (strings(&["removed"]), strings(&["added"])),
                (strings(&["tail"]), strings(&["tail"])),
            ]
        );
    }

    #[test]
    fn hunk_tokens_gives_a_split_row_with_only_one_side_nothing_for_the_other() {
        let hunk = one(&["@@ -1,1 +1,3 @@", " a", "+b", "+c"]);
        let tokens = zipped(&hunk);
        let cells: Vec<_> = to_split_rows(&hunk)
            .into_iter()
            .map(|row| {
                (
                    row.left.map(|i| tokens[i].clone()),
                    row.right.map(|i| tokens[i].clone()),
                )
            })
            .collect();
        assert_eq!(
            cells,
            [
                (Some(strings(&["a"])), Some(strings(&["a"]))),
                (None, Some(strings(&["b"]))),
                (None, Some(strings(&["c"]))),
            ]
        );
    }

    #[test]
    fn language_for_resolves_the_languages_a_review_actually_turns_up() {
        assert_eq!(language_for("src/main/index.ts"), Some("typescript"));
        assert_eq!(language_for("src/App.tsx"), Some("tsx"));
        assert_eq!(language_for("internal/capture.go"), Some("go"));
        assert_eq!(language_for("infra/main.tf"), Some("hcl"));
        assert_eq!(language_for("infra/prod.tfvars"), Some("hcl"));
        assert_eq!(language_for("app/models/user.rb"), Some("ruby"));
        assert_eq!(language_for("src/Controller.php"), Some("php"));
        assert_eq!(language_for("src/lib.rs"), Some("rust"));
        assert_eq!(language_for("scripts/release.sh"), Some("shellscript"));
        assert_eq!(language_for("db/schema.sql"), Some("sql"));
    }

    #[test]
    fn language_for_resolves_the_configuration_formats_every_repository_has() {
        assert_eq!(language_for("deploy/values.YAML"), Some("yaml"));
        assert_eq!(language_for(".github/workflows/ci.yml"), Some("yaml"));
        assert_eq!(language_for("Cargo.toml"), Some("toml"));
        assert_eq!(language_for("tsconfig.json"), Some("json"));
        assert_eq!(language_for(".vscode/settings.jsonc"), Some("jsonc"));
        assert_eq!(language_for("setup.cfg"), Some("ini"));
        assert_eq!(language_for("nginx.conf"), Some("ini"));
        assert_eq!(language_for("gradle.properties"), Some("properties"));
        assert_eq!(language_for("infra/network.bicep"), Some("bicep"));
        assert_eq!(language_for("api/service.proto"), Some("proto"));
    }

    #[test]
    fn language_for_takes_the_last_extension_so_a_compound_name_still_resolves() {
        assert_eq!(language_for("src/shared/types.d.ts"), Some("typescript"));
        assert_eq!(language_for("test/diff.test.ts"), Some("typescript"));
        assert_eq!(language_for("docker-compose.override.yml"), Some("yaml"));
    }

    #[test]
    fn language_for_resolves_well_known_names_that_carry_no_extension() {
        assert_eq!(language_for("Dockerfile"), Some("docker"));
        assert_eq!(language_for("build/Dockerfile.dev"), Some("docker"));
        assert_eq!(language_for("Makefile"), Some("make"));
        assert_eq!(language_for("GNUmakefile"), Some("make"));
        assert_eq!(language_for("Rakefile"), Some("ruby"));
        assert_eq!(language_for("Gemfile"), Some("ruby"));
        assert_eq!(language_for("Jenkinsfile"), Some("groovy"));
        assert_eq!(language_for("CMakeLists.txt"), Some("cmake"));
        assert_eq!(language_for(".zshrc"), Some("shellscript"));
    }

    #[test]
    fn language_for_gives_up_on_anything_else() {
        assert_eq!(language_for("LICENSE"), None);
        assert_eq!(language_for(".gitignore"), None);
        assert_eq!(language_for("assets/logo.sketch"), None);
        assert_eq!(language_for(""), None);
        assert_eq!(language_for("some/dir/"), None);
    }

    #[test]
    fn language_for_keeps_the_typescript_reading_of_a_dotless_name() {
        // `slice(0, -1)`: the name minus its last character is checked as a base.
        assert_eq!(language_for("Makefiles"), Some("make"));
        assert_eq!(language_for("Dockerfileé"), Some("docker"));
        // Non-ASCII names are lowered and cut on character boundaries.
        assert_eq!(language_for("src/ŽLUŤOUČKÝ.RS"), Some("rust"));
        assert_eq!(language_for("ü"), None);
    }

    /// Languages [`language_for`] can name that two-face has no grammar for. They
    /// render plain; this list is here so that one more joining it is a decision.
    const UNSUPPORTED: &[&str] = &[
        "ahk",
        "astro",
        "bicep",
        "cobol",
        "gleam",
        "handlebars",
        "haxe",
        "hlsl",
        "liquid",
        "move",
        "prisma",
        "prolog",
        "pug",
        "razor",
        "v",
        "vb",
        "wasm",
    ];

    #[test]
    fn every_language_the_detector_can_name_has_a_grammar_or_is_known_not_to() {
        // This is the guard that catches a language id that does not exist, which
        // would otherwise show up as one file quietly rendering plain and nothing
        // saying why.
        let unknown: Vec<_> = HIGHLIGHT_LANGUAGES
            .iter()
            .filter(|language| syntax_name(language).is_none())
            .copied()
            .collect();
        assert_eq!(unknown, UNSUPPORTED);

        for path in ["a.ts", "a.tf", "Dockerfile", "Makefile", "a.kt", "a.jsonc"] {
            let language = language_for(path);
            assert!(
                language.is_some_and(|language| HIGHLIGHT_LANGUAGES.contains(&language)),
                "{path} -> {language:?}"
            );
        }
        let mut sorted = HIGHLIGHT_LANGUAGES.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(*HIGHLIGHT_LANGUAGES, sorted);
    }

    #[test]
    fn every_grammar_named_exists_in_the_syntax_set() {
        let loaded = Highlighter::shared().loaded();
        let missing: Vec<_> = SYNTAXES
            .iter()
            .filter(|(_, name)| loaded.syntaxes.find_syntax_by_name(name).is_none())
            .collect();
        assert_eq!(missing, Vec::<&(&str, &str)>::new());
        for (alias, id) in ALIASES {
            assert!(syntax_name(id).is_some(), "{alias} -> {id}");
        }
    }

    #[test]
    fn the_tables_are_sorted_and_free_of_repeats() {
        let sorted = |table: &[(&str, &str)]| table.windows(2).all(|pair| pair[0].0 < pair[1].0);
        assert!(sorted(BY_EXTENSION));
        assert!(sorted(SYNTAXES));
        assert!(sorted(ALIASES));
        assert!(
            ALIASES
                .iter()
                .all(|(alias, _)| SYNTAXES.iter().all(|(id, _)| id != alias))
        );
    }

    #[test]
    fn fence_tags_resolve_through_the_registry_the_extensions_and_the_aliases() {
        assert_eq!(fence_language("ts"), Some("typescript"));
        assert_eq!(fence_language("TypeScript"), Some("typescript"));
        assert_eq!(fence_language("yml"), Some("yaml"));
        assert_eq!(fence_language("c++"), Some("cpp"));
        assert_eq!(fence_language("patch"), Some("diff"));
        assert_eq!(fence_language("conf"), Some("ini"));
        assert_eq!(fence_language("golang"), Some("go"));
        assert_eq!(fence_language("shell-session"), Some("shellscript"));
        assert_eq!(fence_language("prose"), None);
        assert_eq!(fence_language(""), None);
    }

    // --- the highlighter ---

    fn hex(color: &str) -> u32 {
        theme_colour(Some(color)).unwrap_or_else(|| panic!("{color}"))
    }

    fn texts<'a>(line: &'a str, tokens: &[Token]) -> Vec<&'a str> {
        tokens.iter().map(|token| token.text(line)).collect()
    }

    fn colour_of(line: &str, tokens: &[Token], text: &str) -> u32 {
        match tokens.iter().find(|token| token.text(line) == text) {
            Some(token) => token.color,
            None => panic!("no token {text:?} in {:?}", texts(line, tokens)),
        }
    }

    #[test]
    fn highlighting_covers_every_line_and_every_byte() {
        let code = "const a: number = 1 // one\n\nfunction b() {\r\n  return \"ž🐎\"\n}";
        let lines = highlight_lines("typescript", code, false);
        let source: Vec<&str> = code.split('\n').collect();
        assert_eq!(lines.len(), source.len());
        for (line, tokens) in source.iter().zip(&lines) {
            let line = line.strip_suffix('\r').unwrap_or(line);
            assert_eq!(texts(line, tokens).concat(), line);
            for pair in tokens.windows(2) {
                assert_eq!(pair[0].range.end, pair[1].range.start);
            }
        }
        assert_eq!(lines[1], Vec::new());
    }

    #[test]
    fn tokens_take_the_colours_shiki_gave_them() {
        let line = "const answer = \"forty-two\" // a comment";
        let light = highlight_lines("typescript", line, false);
        let dark = highlight_lines("typescript", line, true);

        // storage.type -> keyword red, a string -> navy, a comment -> grey.
        assert_eq!(colour_of(line, &light[0], "const"), hex("#cf222e"));
        // The quotes are the string's colour too, so they are one token with it.
        assert_eq!(colour_of(line, &light[0], "\"forty-two\""), hex("#0a3069"));
        assert_eq!(colour_of(line, &dark[0], "const"), hex("#ff7b72"));
        assert!(
            light[0].iter().any(
                |token| token.text(line).contains("a comment") && token.color == hex("#6e7781")
            )
        );
        // Nothing in TypeScript paints a background.
        assert!(light[0].iter().all(|token| token.background.is_none()));
    }

    #[test]
    fn plain_text_takes_the_editor_foreground() {
        let lines = highlight_lines("markdown", "just words", true);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].color, hex("#e6edf3"));
    }

    #[test]
    fn a_patch_paints_the_theme_background_behind_inserted_and_deleted_lines() {
        let text = "@@ -1 +1 @@\n-old\n+new";
        let light = highlight_lines("diff", text, false);
        assert!(
            light[1]
                .iter()
                .all(|token| token.background == Some(hex("#ffebe9")))
        );
        assert!(
            light[2]
                .iter()
                .all(|token| token.background == Some(hex("#dafbe1")))
        );
        // The marker and the text share colour and background, so they are one token.
        assert_eq!(colour_of("+new", &light[2], "+new"), hex("#116329"));
        let dark = highlight_lines("diff", text, true);
        assert!(
            dark[2]
                .iter()
                .all(|token| token.background == Some(hex("#04260f")))
        );
    }

    #[test]
    fn a_heading_is_bold_in_markdown() {
        let lines = highlight_lines("markdown", "# Title", false);
        assert!(lines[0].iter().any(|token| token.font_style.bold));
        assert!(lines[0].iter().all(|token| token.color == hex("#0550ae")));
    }

    #[test]
    fn an_unknown_language_highlights_nothing() {
        assert_eq!(
            highlight_lines("astro", "<p/>", false),
            Vec::<Vec<Token>>::new()
        );
        assert_eq!(
            highlight_lines("nonsense", "x", false),
            Vec::<Vec<Token>>::new()
        );
        assert_eq!(highlight_code("x", "prose", false), None);
        assert_eq!(highlight_code("<p/>", "astro", false), None);
    }

    #[test]
    fn highlight_code_resolves_the_fence_and_returns_a_line_per_line() {
        let code = "fn main() {}\n";
        let Some(lines) = highlight_code(code, "rs", true) else {
            panic!("rust fences highlight");
        };
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], Vec::new());
        assert_eq!(colour_of("fn main() {}", &lines[0], "fn"), hex("#ff7b72"));
    }

    #[test]
    fn highlight_hunks_puts_each_sides_tokens_on_its_own_lines() {
        let hunks = parse_patch(
            "@@ -1,3 +1,3 @@\n const a = 1\n-let b = \"x\"\n+const b = 2\n\\ No newline at end of file\n@@ -9 +9 @@\n+// tail",
        );
        let tokens = highlight_hunks(&hunks, "typescript", false);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].len(), hunks[0].lines.len());
        for (hunk, lines) in hunks.iter().zip(&tokens) {
            for (line, line_tokens) in hunk.lines.iter().zip(lines) {
                match line.kind {
                    DiffLineKind::Meta => assert_eq!(line_tokens, &None),
                    _ => {
                        let Some(line_tokens) = line_tokens else {
                            panic!("{:?} has no tokens", line.content);
                        };
                        assert_eq!(texts(&line.content, line_tokens).concat(), line.content);
                    }
                }
            }
        }
        let removed = &hunks[0].lines[1];
        let removed_tokens = tokens[0][1].clone().unwrap_or_default();
        assert_eq!(
            colour_of(&removed.content, &removed_tokens, "let"),
            hex("#cf222e")
        );

        let plain = highlight_hunks(&hunks, "astro", false);
        assert!(plain.iter().flatten().all(Option::is_none));
        assert_eq!(plain[0].len(), hunks[0].lines.len());
    }

    #[test]
    fn an_overlong_line_renders_plain_and_leaves_the_rest_highlighted() {
        let long = format!("const x = \"{}\"", "a".repeat(MAX_LINE_BYTES));
        let text = format!("{long}\nconst y = 1");
        let lines = highlight_lines("typescript", &text, false);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].range, 0..long.len());
        assert_eq!(lines[0][0].color, hex("#1f2328"));
        assert_eq!(colour_of("const y = 1", &lines[1], "const"), hex("#cf222e"));
    }

    #[test]
    fn the_theme_trie_resolves_like_vscode_textmate() {
        let theme = CompiledTheme::new(syntax_theme(false));
        let colour = |stack: &[&str]| {
            let mut painter = Painter::new(&theme, false);
            for scope in stack {
                match Scope::new(scope) {
                    Ok(scope) => painter.push(scope),
                    Err(error) => panic!("{scope}: {error:?}"),
                }
            }
            painter.top().0.foreground
        };
        // A deeper scope's rule wins over the enclosing one.
        assert_eq!(
            colour(&["source.ts", "string.quoted.double.ts"]),
            hex("#0a3069")
        );
        // `string variable` is a rule with a parent scope: it applies only inside a string.
        assert_eq!(colour(&["source.ts", "variable.name.ts"]), hex("#953800"));
        assert_eq!(
            colour(&["source.ts", "string.template.ts", "variable.name.ts"]),
            hex("#0550ae")
        );
        // ...and a deeper rule of the scope's own (`variable.other`) still beats it,
        // because depth in the trie is compared before parent selectors are.
        assert_eq!(
            colour(&["source.ts", "string.template.ts", "variable.other.ts"]),
            hex("#1f2328")
        );
        // A scope no rule names inherits from the one it sits in.
        assert_eq!(
            colour(&["source.ts", "comment.line.ts", "meta.unknown.ts"]),
            hex("#6e7781")
        );
        // `entity.name.function` beats `entity.name` beats `entity`.
        assert_eq!(
            colour(&["source.ts", "entity.name.function.ts"]),
            hex("#8250df")
        );
        assert_eq!(
            colour(&["source.ts", "entity.name.type.ts"]),
            hex("#953800")
        );
        assert_eq!(colour(&["source.ts", "entity.other.ts"]), hex("#0550ae"));
        // Nothing at all: the editor's own foreground.
        assert_eq!(colour(&["source.ts"]), hex("#1f2328"));
    }

    #[test]
    fn parent_scope_matching_follows_vscode_textmate() {
        let rule = |parents: &[&str]| parents.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(parents_match(
            &["string.quoted", "source.ts"],
            &rule(&["string"])
        ));
        assert!(parents_match(
            &["meta.x", "string.quoted"],
            &rule(&["string"])
        ));
        assert!(!parents_match(
            &["meta.x", "string.quoted"],
            &rule(&[">", "string"])
        ));
        assert!(parents_match(
            &["string.quoted", "source"],
            &rule(&[">", "string", "source"])
        ));
        assert!(!parents_match(&["source.ts"], &rule(&["string"])));
        assert!(!parents_match(&["stringy"], &rule(&["string"])));
        assert!(parents_match(&[], &rule(&[])));
    }

    #[test]
    fn the_embedded_themes_parse() {
        for dark in [false, true] {
            let theme = syntax_theme(dark);
            assert_eq!(theme.token_colors.len(), 49);
            assert!(theme.colors.contains_key("editor.foreground"));
            assert_eq!(theme.kind, if dark { "dark" } else { "light" });
        }
    }

    #[test]
    fn highlighting_is_safe_from_many_threads_at_once() {
        let highlighter = Highlighter::new();
        std::thread::scope(|scope| {
            for i in 0..4 {
                let highlighter = &highlighter;
                scope.spawn(move || {
                    let lines = highlighter.highlight_lines("rust", "let x = 1;", i % 2 == 0);
                    assert_eq!(lines.len(), 1);
                });
            }
        });
    }

    /// `cargo test --release -p reviewdeck-core -- --ignored highlight_benchmark --nocapture`
    #[test]
    #[ignore = "a benchmark; run it in release"]
    fn highlight_benchmark() {
        let block = TS_BLOCK;
        let text = block.repeat(5000 / block.lines().count() + 1);
        let text: String = text.lines().take(5000).collect::<Vec<_>>().join("\n");
        let highlighter = Highlighter::new();

        let started = std::time::Instant::now();
        highlighter.preload();
        let loaded = started.elapsed();
        let started = std::time::Instant::now();
        let _ = highlighter.highlight_lines("typescript", "const warm = 1", false);
        let warmed = started.elapsed();

        for language in ["typescript", "rust", "python", "diff"] {
            let started = std::time::Instant::now();
            let lines = highlighter.highlight_lines(language, &text, false);
            let took = started.elapsed();
            assert_eq!(lines.len(), 5000);
            eprintln!("{language}: 5000 lines in {took:?}");
        }
        eprintln!("load {loaded:?}, first typescript grammar {warmed:?}");

        // For comparison: one parser in order, and a text that never returns to
        // the top level (so the lanes cannot help).
        if let Some((loaded, syntax)) = highlighter.syntax("typescript") {
            let context = Context {
                loaded,
                syntax,
                theme: &loaded.light,
                backgrounds: false,
            };
            let lines: Vec<&str> = text.split('\n').collect();
            let started = std::time::Instant::now();
            let _ = tokenize_lines(&context, &lines, 1, 1);
            eprintln!("typescript in order: {:?}", started.elapsed());
        }
        let nested = format!("class Everything {{\n{text}\n}}");
        let started = std::time::Instant::now();
        let _ = highlighter.highlight_lines("typescript", &nested, false);
        eprintln!("typescript inside one class: {:?}", started.elapsed());

        let started = std::time::Instant::now();
        let lines = highlighter.highlight_lines("typescript", &text, true);
        let took = started.elapsed();
        eprintln!("typescript again, warm, dark: {took:?}");
        assert_eq!(lines.len(), 5000);
        assert!(
            took < std::time::Duration::from_millis(100),
            "5000 lines of TypeScript took {took:?}"
        );
    }

    /// The lanes are an optimisation and nothing else: whatever the text, they
    /// must give exactly what one parser going through it in order gives.
    fn lanes_match_one_parser(language: &str, text: &str) {
        let Some((loaded, syntax)) = Highlighter::shared().syntax(language) else {
            panic!("no grammar for {language}");
        };
        for dark in [false, true] {
            let context = Context {
                loaded,
                syntax,
                theme: if dark { &loaded.dark } else { &loaded.light },
                backgrounds: paints_backgrounds(language),
            };
            let lines: Vec<&str> = text.split('\n').collect();
            let Some(in_order) = tokenize_lines(&context, &lines, 1, 1) else {
                panic!("{language} failed to parse");
            };
            assert_eq!(in_order.len(), lines.len());
            for lanes in [2, 3, 5, 8, 13] {
                assert!(
                    tokenize_lines(&context, &lines, lanes, 3).as_ref() == Some(&in_order),
                    "{language} in {lanes} lanes differs from one parser"
                );
            }
        }
    }

    const TS_BLOCK: &str = r#"import { thing, other } from './module'

/** A documented function. */
export async function handle(request: Request, options?: { retries: number }): Promise<Response> {
  const url = new URL(request.url)
  if (url.pathname.startsWith('/api/')) {
    for (let attempt = 0; attempt < (options?.retries ?? 3); attempt++) {
      const response = await fetch(`${url.origin}/upstream${url.pathname}`, { method: 'GET' })
      if (response.ok) return response // done
    }
  }
  return new Response(JSON.stringify({ error: "not found", code: 404 }), { status: 404 })
}
"#;

    #[test]
    fn lanes_match_one_parser_on_code_that_keeps_returning_to_the_top_level() {
        let text = TS_BLOCK.repeat(30);
        lanes_match_one_parser("typescript", &text);

        // ...and the lanes did the work: nearly everything after the first lane came
        // from the lanes that started from a guess.
        let Some((loaded, syntax)) = Highlighter::shared().syntax("typescript") else {
            panic!("no TypeScript grammar");
        };
        let context = Context {
            loaded,
            syntax,
            theme: &loaded.light,
            backgrounds: false,
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let Some((_, guessed)) = tokenize_in_lanes(&context, &lines, 4, 2) else {
            panic!("TypeScript failed to parse");
        };
        assert!(
            guessed * 2 > lines.len(),
            "only {guessed} of {} lines",
            lines.len()
        );
    }

    #[test]
    fn lanes_match_one_parser_on_code_that_never_leaves_a_block() {
        let body = TS_BLOCK
            .replace("export async function", "async")
            .replace("import", "// import");
        lanes_match_one_parser(
            "typescript",
            &format!("class Everything {{\n{}}}\n", body.repeat(30)),
        );
    }

    #[test]
    fn lanes_match_one_parser_across_comments_and_strings_spanning_their_starts() {
        let filler = "const x = 1\n".repeat(150);
        lanes_match_one_parser(
            "typescript",
            &format!("/*\n{filler}*/\n{filler}const s = `\n{filler}`\n{filler}"),
        );
        let python = "def f():\n    return 1\n".repeat(80);
        lanes_match_one_parser("python", &format!("\"\"\"\n{python}\"\"\"\n{python}"));
        let markdown = "# Title\n\nSome *text*.\n\n```ts\nconst a = 1\n```\n\n- item\n".repeat(60);
        lanes_match_one_parser("markdown", &markdown);
        let patch = "@@ -1,3 +1,3 @@\n context\n-old\n+new\n".repeat(120);
        lanes_match_one_parser("diff", &patch);
    }

    #[test]
    fn a_big_text_is_highlighted_in_lanes_with_the_same_result() {
        let text = TS_BLOCK.repeat(60);
        assert!(text.split('\n').count() >= PARALLEL_MIN_LINES);
        let highlighted = highlight_lines("typescript", &text, false);
        let Some((loaded, syntax)) = Highlighter::shared().syntax("typescript") else {
            panic!("no TypeScript grammar");
        };
        let context = Context {
            loaded,
            syntax,
            theme: &loaded.light,
            backgrounds: false,
        };
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(Some(highlighted), tokenize_lines(&context, &lines, 1, 1));
    }
}
