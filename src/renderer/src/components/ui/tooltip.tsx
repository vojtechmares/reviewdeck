import { useState, type ReactNode } from 'react'
import { cn } from '@/lib/utils'

/** Hover/focus hint. Positioned with plain CSS so there is no popper dependency. */
export function Tooltip({
  label,
  children,
  side = 'bottom',
  className,
}: {
  label: ReactNode
  children: ReactNode
  side?: 'top' | 'bottom'
  className?: string
}): React.JSX.Element {
  const [open, setOpen] = useState(false)

  return (
    <span
      className="relative inline-flex"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
      onFocus={() => setOpen(true)}
      onBlur={() => setOpen(false)}
    >
      {children}
      {open && (
        <span
          role="tooltip"
          className={cn(
            'glass-overlay pointer-events-none absolute left-1/2 z-50 -translate-x-1/2 rounded-md px-2 py-1',
            'max-w-72 text-[11.5px] leading-snug font-medium text-foreground whitespace-pre-line',
            side === 'bottom' ? 'top-[calc(100%+6px)]' : 'bottom-[calc(100%+6px)]',
            className,
          )}
        >
          {label}
        </span>
      )}
    </span>
  )
}
