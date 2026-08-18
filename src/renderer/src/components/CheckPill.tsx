import { CheckCircle2, CircleDashed, Loader2, XCircle } from 'lucide-react'
import type { CheckStatus, CheckSummary } from '@shared/types'
import { cn } from '@/lib/utils'
import { Badge } from './ui/badge'
import { Popover, PopoverContent, PopoverTrigger } from './ui/popover'

const META: Record<CheckStatus, { tone: 'ok' | 'bad' | 'busy' | 'neutral'; label: string }> = {
  passed: { tone: 'ok', label: 'Passed' },
  failed: { tone: 'bad', label: 'Failed' },
  running: { tone: 'busy', label: 'Running' },
  unknown: { tone: 'neutral', label: 'Unknown' },
}

/** The colour a status is drawn in wherever a run is listed rather than rolled up. */
export const CHECK_TONE: Record<CheckStatus, string> = {
  passed: 'text-ok',
  failed: 'text-bad',
  running: 'text-busy',
  unknown: 'text-muted-foreground',
}

export function CheckIcon({
  status,
  className = 'size-3.5',
}: {
  status: CheckStatus
  className?: string
}): React.JSX.Element {
  if (status === 'passed') return <CheckCircle2 className={className} />
  if (status === 'failed') return <XCircle className={className} />
  if (status === 'running') return <Loader2 className={`${className} spin`} />
  return <CircleDashed className={className} />
}

/**
 * Compact CI badge; opening it lists the runs behind the roll-up.
 *
 * The badge is the trigger rather than a button wrapping it, because one of the two
 * places this appears is inside the button a review card already is, and a button
 * cannot contain another one. That costs the keyboard handling a real button would
 * have given for free, which is what the role, the tab stop and the key handler
 * below put back.
 */
export function CheckPill({ checks }: { checks: CheckSummary }): React.JSX.Element {
  const meta = META[checks.status]

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Badge
          tone={meta.tone}
          role="button"
          tabIndex={0}
          aria-label={`Checks: ${meta.label.toLowerCase()}`}
          className="cursor-pointer transition-opacity hover:opacity-80"
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => {
            if (event.key !== 'Enter' && event.key !== ' ') return
            event.preventDefault()
            event.currentTarget.click()
          }}
        >
          <CheckIcon status={checks.status} className="size-3" />
          {checks.total > 0 ? (
            <span>
              {checks.status === 'failed'
                ? `${checks.failed} failed`
                : checks.status === 'running'
                  ? `${checks.running} running`
                  : `${checks.passed}/${checks.total}`}
            </span>
          ) : (
            <span>{meta.label}</span>
          )}
        </Badge>
      </PopoverTrigger>

      <PopoverContent className="w-72">
        {checks.total === 0 ? (
          <p className="px-3 py-2.5 text-[11.5px] leading-snug text-muted-foreground">
            No status checks reported for this branch.
          </p>
        ) : (
          <>
            <div className="flex items-center gap-2 border-b border-border px-3 py-2">
              <CheckIcon
                status={checks.status}
                className={cn('size-3.5 shrink-0', CHECK_TONE[checks.status])}
              />
              <p className="text-[12px] font-medium">{meta.label}</p>
              <span className="ml-auto text-[11px] tabular-nums text-muted-foreground">
                {checks.passed}/{checks.total} passed
              </span>
            </div>

            <ul className="divide-y divide-border">
              {checks.runs.map((run) => (
                <li key={run.id} className="flex items-start gap-2 px-3 py-1.5">
                  <CheckIcon
                    status={run.status}
                    className={cn('mt-0.5 size-3.5 shrink-0', CHECK_TONE[run.status])}
                  />
                  <div className="min-w-0">
                    <p className="truncate text-[12px] leading-snug font-medium">{run.name}</p>
                    {run.description && (
                      <p className="truncate text-[11px] leading-snug text-muted-foreground">
                        {run.description}
                      </p>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          </>
        )}
      </PopoverContent>
    </Popover>
  )
}
