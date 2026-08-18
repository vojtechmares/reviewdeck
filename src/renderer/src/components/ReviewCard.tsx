import { useEffect, useRef } from 'react'
import { CheckCheck, FileDiff, GitPullRequestArrow, MessageSquare, XOctagon } from 'lucide-react'
import type { Account, ReviewItem } from '@shared/types'
import { cn, formatCount, relativeTime } from '@/lib/utils'
import { Avatar } from './ui/avatar'
import { Badge } from './ui/badge'
import { CheckPill } from './CheckPill'
import { ProviderIcon } from './ProviderIcon'

interface ReviewCardProps {
  item: ReviewItem
  account?: Account
  selected: boolean
  onSelect: () => void
}

export function ReviewCard({ item, account, selected, onSelect }: ReviewCardProps): React.JSX.Element {
  const node = useRef<HTMLButtonElement>(null)

  // j/k moves the selection without touching the scroll position, so follow it.
  useEffect(() => {
    if (selected) node.current?.scrollIntoView({ block: 'nearest' })
  }, [selected])

  return (
    <button
      ref={node}
      onClick={onSelect}
      aria-current={selected}
      className={cn(
        'group w-full rounded-lg border px-3 py-2.5 text-left transition-all duration-150',
        selected
          ? 'border-border-strong bg-surface-strong shadow-[0_1px_0_0_var(--highlight)_inset]'
          : 'border-transparent hover:border-border hover:bg-surface-muted',
      )}
    >
      <div className="flex items-start gap-2.5">
        <Avatar src={item.author.avatarUrl} name={item.author.name} className="mt-0.5 size-6.5" />

        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-1.5 text-[11.5px] text-muted-foreground">
            <ProviderIcon kind={item.provider} className="size-3 translate-y-px opacity-70" />
            <span className="truncate font-medium">{item.repo}</span>
            <span className="opacity-60">#{item.number}</span>
            <span className="ml-auto shrink-0 tabular-nums opacity-70">
              {relativeTime(item.updatedAt)}
            </span>
          </div>

          <p
            className={cn(
              'mt-0.5 line-clamp-2 text-[13px] leading-snug',
              selected ? 'font-semibold' : 'font-medium',
            )}
          >
            {item.title}
          </p>

          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <CheckPill checks={item.checks} />

            {item.draft && (
              <Badge>
                <GitPullRequestArrow className="size-3" />
                Draft
              </Badge>
            )}

            {item.myReviewState === 'approved' && (
              <Badge tone="ok">
                <CheckCheck className="size-3" />
                You approved
              </Badge>
            )}
            {item.myReviewState === 'changes_requested' && (
              <Badge tone="bad">
                <XOctagon className="size-3" />
                Changes requested
              </Badge>
            )}
            {item.myReviewState === 'commented' && (
              <Badge tone="info">
                <MessageSquare className="size-3" />
                Commented
              </Badge>
            )}

            {item.changedFiles !== undefined && (
              <Badge>
                <FileDiff className="size-3" />
                {formatCount(item.changedFiles)}
                <span className="text-ok">+{formatCount(item.additions)}</span>
                <span className="text-bad">−{formatCount(item.deletions)}</span>
              </Badge>
            )}

            {account && (
              <span className="ml-auto max-w-28 truncate text-[10.5px] text-muted-foreground/80">
                {account.label}
              </span>
            )}
          </div>
        </div>
      </div>
    </button>
  )
}
