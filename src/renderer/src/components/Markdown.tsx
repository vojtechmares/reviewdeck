import {
  Children,
  cloneElement,
  createContext,
  Fragment,
  isValidElement,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import {
  Info,
  Lightbulb,
  MessageSquareWarning,
  OctagonAlert,
  TriangleAlert,
  type LucideIcon,
} from 'lucide-react'
import ReactMarkdown, { type Components, type ExtraProps } from 'react-markdown'
import type { PluggableList } from 'unified'
import {
  ALERT_MARKER,
  alertKindOf,
  REHYPE_PLUGINS,
  REMARK_PLUGINS,
  rehypeAuthenticatedImages,
  remarkAutolink,
} from '@shared/markdown'
import type { AlertKind, AutolinkContext } from '@shared/markdown'
import type { ImageContext } from '@shared/images'
import type { Element } from 'hast'
import { highlightCode, type Token } from '@/lib/highlight'
import { cn } from '@/lib/utils'

/**
 * GitHub Alerts, in the app's own status colours rather than GitHub's.
 *
 * There are five kinds and four status tokens, so a note and an important share
 * the informational blue and are told apart by their icon and label - which is
 * how they read anyway, both being informational rather than a problem.
 */
const ALERTS: Record<AlertKind, { label: string; icon: LucideIcon; box: string; tone: string }> = {
  note: { label: 'Note', icon: Info, box: 'border-info/30 bg-info-soft', tone: 'text-info' },
  tip: { label: 'Tip', icon: Lightbulb, box: 'border-ok/30 bg-ok-soft', tone: 'text-ok' },
  important: {
    label: 'Important',
    icon: MessageSquareWarning,
    box: 'border-info/30 bg-info-soft',
    tone: 'text-info',
  },
  warning: {
    label: 'Warning',
    icon: TriangleAlert,
    box: 'border-busy/30 bg-busy-soft',
    tone: 'text-busy',
  },
  caution: {
    label: 'Caution',
    icon: OctagonAlert,
    box: 'border-bad/30 bg-bad-soft',
    tone: 'text-bad',
  },
}

/**
 * A block quote, or a callout when its first line is nothing but an alert marker.
 *
 * An ordinary block quote - and one whose marker is misspelt or names a kind that
 * does not exist - falls through to the block quote it looks like, rather than
 * rendering as a broken callout.
 */
function Blockquote({
  node,
  children,
  ...props
}: React.JSX.IntrinsicElements['blockquote'] & ExtraProps): React.JSX.Element {
  const kind = node ? alertKindOf(node) : null
  if (!kind) return <blockquote {...props}>{children}</blockquote>

  const alert = ALERTS[kind]
  const Icon = alert.icon

  return (
    <div className={cn('my-[0.7em] rounded-md border px-3 py-2.5', alert.box)}>
      <p className={cn('mb-1 flex items-center gap-1.5 text-[12px] font-semibold', alert.tone)}>
        <Icon className="size-3.5 shrink-0" />
        {alert.label}
      </p>
      <div className="[&>:first-child]:mt-0 [&>:last-child]:mb-0">
        {withoutMarker(children)}
      </div>
    </div>
  )
}

/**
 * Drops the marker line from the rendered body.
 *
 * The marker is the leading text of the block quote's first paragraph, and it is
 * either followed by the body in that same paragraph or alone in one of its own -
 * a blank quoted line between the two makes the difference. In the second case
 * stripping empties the paragraph, so it goes too.
 */
function withoutMarker(children: ReactNode): ReactNode {
  let stripped = false

  return Children.toArray(children).flatMap((child) => {
    if (stripped || !isValidElement<{ children?: ReactNode }>(child)) return [child]
    stripped = true

    const inner = Children.toArray(child.props.children)
    const first = inner[0]
    if (typeof first !== 'string') return [child]

    const rest = first.replace(ALERT_MARKER, '')
    if (!rest && inner.length === 1) return []
    inner[0] = rest

    return [cloneElement(child, undefined, inner)]
  })
}

const FENCE_LANGUAGE = /(?:^|\s)language-([\w.+#-]+)/

/** Everything a hast subtree says, which for a fenced block is its source. */
function textOf(node: Element | undefined): string {
  if (!node) return ''
  return node.children
    .map((child) =>
      child.type === 'text' ? child.value : child.type === 'element' ? textOf(child) : '',
    )
    .join('')
}

/**
 * Inline code, and fences with no language tag, keep the preformatted rendering
 * they already had. Only a tagged fence goes to the highlighter, and it does so in
 * a component of its own so an ordinary code span costs nothing.
 */
function Code({
  node,
  className,
  children,
  ...props
}: React.JSX.IntrinsicElements['code'] & ExtraProps): React.JSX.Element {
  const tag = FENCE_LANGUAGE.exec(className ?? '')?.[1]
  if (!tag) {
    return (
      <code className={className} {...props}>
        {children}
      </code>
    )
  }

  return (
    <HighlightedCode tag={tag} source={textOf(node)} className={className} {...props}>
      {children}
    </HighlightedCode>
  )
}

function HighlightedCode({
  tag,
  source,
  className,
  children,
  ...props
}: React.JSX.IntrinsicElements['code'] & {
  tag: string
  source: string
}): React.JSX.Element {
  const [tokens, setTokens] = useState<Token[][] | null>(null)

  // Colour arrives after the block does, exactly as it does in the diff: a snippet
  // is readable the moment it renders, and a grammar never stands in front of it.
  useEffect(() => {
    setTokens(null)
    if (!source) return

    let cancelled = false
    void highlightCode(source.replace(/\n$/, ''), tag).then((result) => {
      if (!cancelled) setTokens(result)
    })
    return () => {
      cancelled = true
    }
  }, [source, tag])

  if (!tokens) {
    return (
      <code className={className} {...props}>
        {children}
      </code>
    )
  }

  return (
    <code className={className} {...props}>
      {tokens.map((line, index) => (
        <Fragment key={index}>
          {index > 0 && '\n'}
          {line.map((token, position) => (
            <span key={position} className="tok" style={token.style as React.CSSProperties}>
              {token.content}
            </span>
          ))}
        </Fragment>
      ))}
    </code>
  )
}

const COMPONENTS: Components = { blockquote: Blockquote, code: Code }

/** Everything a body needs to resolve against the pull request it belongs to. */
export type MarkdownTargets = AutolinkContext & ImageContext

/**
 * Which host a body was written on, so `@someone`, `#123` and a relative image
 * source resolve there.
 *
 * A context rather than a prop, because markdown is rendered several levels down -
 * a comment inside a thread inside a diff row - and every one of those surfaces
 * belongs to the same pull request anyway.
 */
const Targets = createContext<MarkdownTargets | null>(null)

export function MarkdownLinks({
  value,
  children,
}: {
  value: MarkdownTargets
  children: ReactNode
}): React.JSX.Element {
  return <Targets.Provider value={value}>{children}</Targets.Provider>
}

/**
 * Every markdown surface in the app renders through here - descriptions, comments,
 * and whatever comes next - so a body reads the same wherever it appears.
 *
 * Links need no handler: the main process blocks in-page navigation and hands HTTP
 * and HTTPS to the system browser, so an ordinary anchor already does the right
 * thing.
 */
export function Markdown({
  children,
  compact,
  className,
}: {
  children: string
  /**
   * For a comment living inside a diff row. Same parser and same plugins - only
   * the scale and the containment differ, so a fenced code suggestion still
   * renders where people actually leave them.
   */
  compact?: boolean
  className?: string
}): React.JSX.Element {
  const targets = useContext(Targets)
  // Emoji renders with or without a host; only the links need one.
  const remarkPlugins = useMemo<PluggableList>(
    () => [...REMARK_PLUGINS, [remarkAutolink, targets]],
    [targets],
  )
  // Last, after the sanitizer: it only permits http and https on an image source.
  const rehypePlugins = useMemo<PluggableList>(
    () => [...REHYPE_PLUGINS, [rehypeAuthenticatedImages, targets]],
    [targets],
  )

  return (
    <div className={cn('md', compact && 'md-compact', className)}>
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={rehypePlugins}
        components={COMPONENTS}
      >
        {children}
      </ReactMarkdown>
    </div>
  )
}
