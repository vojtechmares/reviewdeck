/**
 * The markdown pipeline, configured once.
 *
 * The renderer hands these plugin lists to `react-markdown`, which wraps them in
 * `remark-parse` -> remark plugins -> `remark-rehype` -> rehype plugins. The tests
 * build the same processor by hand, so the wiring asserted there is the wiring
 * that ships.
 *
 * No HTML string ever reaches the DOM: rendering produces React elements from the
 * sanitized tree, so the sanitizer is the only thing standing between a pull
 * request body written by someone else and the app.
 */
import rehypeRaw from 'rehype-raw'
import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from 'rehype-sanitize'
import remarkGfm from 'remark-gfm'
import type { PluggableList } from 'unified'

/**
 * Elements real pull request bodies lean on: collapsible sections for long bot
 * reports, and keyboard keys in instructions.
 *
 * The library's GitHub-derived default already permits all three. They are named
 * here, and asserted in the tests, so an upstream narrowing cannot quietly turn a
 * collapsed compatibility table back into a wall of text.
 */
const PINNED_TAG_NAMES = ['details', 'summary', 'kbd']

/**
 * Raw HTML is allowed through this rather than stripped: bodies and bot comments
 * use collapsible sections, sized images and line breaks inside table cells, and
 * markdown without HTML would render those worse than the plain text they replace.
 *
 * Ids stay behind the default `user-content-` clobber prefix. The app looks
 * elements up by id, so an id borrowed from a pull request description must not be
 * able to shadow one of the app's own. The visible cost is that GFM footnote
 * anchors, which arrive already prefixed, end up prefixed twice and so do not
 * resolve - cosmetic, and worth it.
 */
export const SANITIZE_SCHEMA: SanitizeSchema = {
  ...defaultSchema,
  tagNames: [...new Set([...(defaultSchema.tagNames ?? []), ...PINNED_TAG_NAMES])],
  /*
   * `input` is permitted so GFM task lists keep their checkboxes. Stripping the
   * attributes off some other input leaves a bare one behind, which is a text box
   * - a credential prompt sitting in someone else's prose. Forcing the two
   * properties back on makes anything that survives a disabled checkbox.
   */
  required: {
    ...defaultSchema.required,
    input: { ...defaultSchema.required?.['input'], type: 'checkbox', disabled: true },
  },
}

export const REMARK_PLUGINS: PluggableList = [remarkGfm]

/** Order matters: raw HTML has to become a tree before the sanitizer can prune it. */
export const REHYPE_PLUGINS: PluggableList = [rehypeRaw, [rehypeSanitize, SANITIZE_SCHEMA]]
