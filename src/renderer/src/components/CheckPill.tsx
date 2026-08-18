import { CheckCircle2, CircleDashed, Loader2, XCircle } from 'lucide-react'
import type { CheckStatus, CheckSummary } from '@shared/types'
import { Badge } from './ui/badge'
import { Tooltip } from './ui/tooltip'

const META: Record<CheckStatus, { tone: 'ok' | 'bad' | 'busy' | 'neutral'; label: string }> = {
  passed: { tone: 'ok', label: 'Passed' },
  failed: { tone: 'bad', label: 'Failed' },
  running: { tone: 'busy', label: 'Running' },
  unknown: { tone: 'neutral', label: 'Unknown' },
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

/** Compact CI badge; the tooltip lists the individual checks behind the roll-up. */
export function CheckPill({ checks }: { checks: CheckSummary }): React.JSX.Element {
  const meta = META[checks.status]

  const detail =
    checks.total === 0
      ? 'No checks reported for this branch.'
      : checks.runs
          .slice(0, 10)
          .map((run) => `${symbolFor(run.status)}  ${run.name}`)
          .join('\n') + (checks.runs.length > 10 ? `\n… ${checks.runs.length - 10} more` : '')

  return (
    <Tooltip label={detail}>
      <Badge tone={meta.tone}>
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
    </Tooltip>
  )
}

function symbolFor(status: CheckStatus): string {
  if (status === 'passed') return '✓'
  if (status === 'failed') return '✕'
  if (status === 'running') return '◌'
  return '·'
}
