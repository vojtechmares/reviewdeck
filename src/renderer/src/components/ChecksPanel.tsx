import type { CheckSummary } from '@shared/types'
import { cn } from '@/lib/utils'
import { CHECK_TONE, CheckIcon } from './CheckPill'

export function ChecksPanel({
  checks,
  onOpen,
}: {
  checks: CheckSummary
  onOpen: (url: string) => void
}): React.JSX.Element {
  if (!checks.total) {
    return (
      <p className="px-4 py-6 text-center text-[12.5px] text-muted-foreground">
        No status checks reported for this branch.
      </p>
    )
  }

  return (
    <div className="p-4">
      <div className="glass overflow-hidden rounded-lg">
        <div className="flex items-center gap-3 border-b border-border px-3.5 py-2.5">
          <CheckIcon status={checks.status} className={cn('size-4', CHECK_TONE[checks.status])} />
          <p className="text-[13px] font-medium">
            {checks.status === 'running'
              ? `${checks.running} check${checks.running === 1 ? '' : 's'} still running`
              : checks.status === 'failed'
                ? `${checks.failed} check${checks.failed === 1 ? '' : 's'} failing`
                : checks.status === 'passed'
                  ? 'All checks passed'
                  : 'Check status unknown'}
          </p>
          <span className="ml-auto text-[11.5px] tabular-nums text-muted-foreground">
            {checks.passed}/{checks.total} passed
          </span>
        </div>

        <ul className="divide-y divide-border">
          {checks.runs.map((run) => (
            <li key={run.id} className="flex items-center gap-2.5 px-3.5 py-2">
              <CheckIcon status={run.status} className={cn('size-3.5 shrink-0', CHECK_TONE[run.status])} />
              <div className="min-w-0 flex-1">
                <p className="truncate text-[12.5px] font-medium">{run.name}</p>
                {run.description && (
                  <p className="truncate text-[11.5px] text-muted-foreground">{run.description}</p>
                )}
              </div>
              {run.url && (
                <button
                  onClick={() => onOpen(run.url as string)}
                  className="shrink-0 text-[11.5px] text-info transition-opacity hover:opacity-75"
                >
                  Details
                </button>
              )}
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
