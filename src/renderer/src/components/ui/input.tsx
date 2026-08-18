import { forwardRef, type InputHTMLAttributes, type TextareaHTMLAttributes } from 'react'
import { cn } from '@/lib/utils'

const FIELD =
  'w-full rounded-lg border border-border bg-surface-strong px-3 text-[13px] text-foreground ' +
  'placeholder:text-muted-foreground/70 transition-colors ' +
  'focus:border-border-strong focus:outline-none focus-visible:outline-2 focus-visible:outline-ring ' +
  'disabled:opacity-50'

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(
  function Input({ className, ...props }, ref) {
    return <input ref={ref} className={cn(FIELD, 'no-drag h-9', className)} {...props} />
  },
)

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function Textarea({ className, ...props }, ref) {
    return (
      <textarea ref={ref} className={cn(FIELD, 'no-drag resize-y py-2 leading-relaxed', className)} {...props} />
    )
  },
)

export function Label({
  className,
  ...props
}: React.LabelHTMLAttributes<HTMLLabelElement>): React.JSX.Element {
  return (
    <label
      className={cn('mb-1.5 block text-[12px] font-medium text-muted-foreground', className)}
      {...props}
    />
  )
}

export function Select({
  className,
  ...props
}: React.SelectHTMLAttributes<HTMLSelectElement>): React.JSX.Element {
  return (
    <select
      className={cn(
        FIELD,
        'no-drag h-9 appearance-none bg-[length:14px] bg-[right_0.65rem_center] bg-no-repeat pr-8',
        className,
      )}
      style={{
        backgroundImage:
          "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='%23888' stroke-width='1.6' stroke-linecap='round'%3E%3Cpath d='M4 6.5 8 10.5 12 6.5'/%3E%3C/svg%3E\")",
      }}
      {...props}
    />
  )
}
