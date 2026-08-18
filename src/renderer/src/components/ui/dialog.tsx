import { useEffect, useRef, type ReactNode } from 'react'
import { X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Button } from './button'

interface DialogProps {
  open: boolean
  onClose: () => void
  title: string
  description?: string
  children: ReactNode
  footer?: ReactNode
  className?: string
}

/**
 * A focus-trapping modal built on the platform: no dependency, but Escape,
 * backdrop clicks and Tab cycling all behave the way people expect.
 */
export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  className,
}: DialogProps): React.JSX.Element | null {
  const panel = useRef<HTMLDivElement>(null)
  const restoreFocus = useRef<Element | null>(null)

  useEffect(() => {
    if (!open) return
    restoreFocus.current = document.activeElement

    const focusables = (): HTMLElement[] =>
      Array.from(
        panel.current?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      )

    // Land on the first meaningful control rather than the close button.
    const first = focusables().find((element) => !element.dataset.dialogClose)
    first?.focus()

    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.stopPropagation()
        onClose()
        return
      }
      if (event.key !== 'Tab') return
      const items = focusables()
      if (!items.length) return
      const active = document.activeElement as HTMLElement
      const index = items.indexOf(active)
      const next = event.shiftKey ? index - 1 : index + 1
      if (next < 0 || next >= items.length || index === -1) {
        event.preventDefault()
        items[event.shiftKey ? items.length - 1 : 0].focus()
      }
    }

    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('keydown', onKey, true)
      ;(restoreFocus.current as HTMLElement | null)?.focus?.()
    }
  }, [open, onClose])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-8"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div className="absolute inset-0 bg-black/25 backdrop-blur-[3px]" />
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cn(
          'glass-strong rise relative flex max-h-full w-full max-w-lg flex-col rounded-xl',
          className,
        )}
      >
        <div className="flex items-start gap-3 px-5 pt-4.5 pb-3">
          <div className="min-w-0 flex-1">
            <h2 className="text-[15px] leading-tight font-semibold">{title}</h2>
            {description && (
              <p className="mt-1 text-[12.5px] leading-relaxed text-muted-foreground">{description}</p>
            )}
          </div>
          <Button
            variant="ghost"
            size="icon"
            onClick={onClose}
            data-dialog-close="true"
            aria-label="Close"
            className="-mt-1 -mr-1"
          >
            <X className="size-4" />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-4">{children}</div>
        {footer && (
          <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3.5">
            {footer}
          </div>
        )}
      </div>
    </div>
  )
}
