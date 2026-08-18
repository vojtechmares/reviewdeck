import ReactMarkdown from 'react-markdown'
import { REHYPE_PLUGINS, REMARK_PLUGINS } from '@shared/markdown'
import { cn } from '@/lib/utils'

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
  return (
    <div className={cn('md', compact && 'md-compact', className)}>
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} rehypePlugins={REHYPE_PLUGINS}>
        {children}
      </ReactMarkdown>
    </div>
  )
}
