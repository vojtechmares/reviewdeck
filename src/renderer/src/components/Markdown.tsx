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
  className,
}: {
  children: string
  className?: string
}): React.JSX.Element {
  return (
    <div className={cn('md', className)}>
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} rehypePlugins={REHYPE_PLUGINS}>
        {children}
      </ReactMarkdown>
    </div>
  )
}
