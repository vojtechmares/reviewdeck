import { UserCheck } from 'lucide-react'
import type { ApprovalSummary } from '@shared/types'
import { Badge } from './ui/badge'
import { Tooltip } from './ui/tooltip'

/**
 * Where the pull request stands on approvals, whoever gave them.
 *
 * Grey is a host that asks for none, or one that would not say - a branch with no
 * protection and a token that cannot read it look the same from here. Amber is a
 * host still waiting on somebody, green is one that has what it wants; the number
 * is what has been given, over what is asked for when the host puts a figure on it.
 */
export function ApprovalBadge({ approvals }: { approvals: ApprovalSummary }): React.JSX.Element {
  const tone =
    approvals.outcome === 'satisfied' ? 'ok' : approvals.outcome === 'pending' ? 'busy' : 'neutral'
  const count =
    approvals.required !== undefined && approvals.required > 0
      ? `${approvals.given}/${approvals.required}`
      : String(approvals.given)
  const state =
    approvals.outcome === 'none_required'
      ? `${plural(approvals.given, 'approval')} given. The host requires none, or will not say what it requires.`
      : approvals.outcome === 'satisfied'
        ? `${plural(approvals.given, 'approval')} given. Every approval the host requires is there.`
        : approvals.required !== undefined
          ? `${approvals.given} of ${approvals.required} required approvals given. Still waiting on somebody.`
          : `${plural(approvals.given, 'approval')} given. Still short of what the host requires.`
  return (
    <Tooltip label={`Approvals from any reviewer\n${state}`}>
      <Badge tone={tone} className="tabular-nums">
        <UserCheck className="size-3" />
        {count}
      </Badge>
    </Tooltip>
  )
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`
}
