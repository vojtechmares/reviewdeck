import { forwardRef, type HTMLAttributes } from 'react'
import { cn } from '@/lib/utils'

type Tone = 'neutral' | 'ok' | 'bad' | 'busy' | 'info'

const TONES: Record<Tone, string> = {
  neutral: 'bg-muted text-muted-foreground border-border',
  ok: 'bg-ok-soft text-ok border-ok/25',
  bad: 'bg-bad-soft text-bad border-bad/25',
  busy: 'bg-busy-soft text-busy border-busy/25',
  info: 'bg-info-soft text-info border-info/25',
}

/** Forwards its ref so it can be handed to a primitive as the thing that opens it. */
export const Badge = forwardRef<HTMLSpanElement, HTMLAttributes<HTMLSpanElement> & { tone?: Tone }>(
  function Badge({ className, tone = 'neutral', ...props }, ref) {
    return (
      <span
        ref={ref}
        className={cn(
          'inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5',
          'text-[11px] leading-none font-medium whitespace-nowrap',
          TONES[tone],
          className,
        )}
        {...props}
      />
    )
  },
)
