import { forwardRef, type ButtonHTMLAttributes } from 'react'
import { cn } from '@/lib/utils'

type Variant = 'default' | 'secondary' | 'ghost' | 'outline' | 'success' | 'danger' | 'subtle'
type Size = 'sm' | 'md' | 'lg' | 'icon'

const VARIANTS: Record<Variant, string> = {
  default:
    'bg-primary text-primary-foreground hover:opacity-90 shadow-sm border border-transparent',
  secondary:
    'bg-surface-strong text-foreground border border-border hover:bg-accent backdrop-blur-md',
  ghost: 'text-muted-foreground hover:text-foreground hover:bg-accent border border-transparent',
  outline: 'border border-border-strong text-foreground hover:bg-accent bg-transparent',
  success: 'bg-ok-soft text-ok border border-ok/30 hover:bg-ok/25',
  danger: 'bg-bad-soft text-bad border border-bad/30 hover:bg-bad/25',
  subtle: 'bg-muted text-foreground border border-transparent hover:bg-accent',
}

const SIZES: Record<Size, string> = {
  sm: 'h-7 px-2.5 text-[12px] gap-1.5 rounded-md',
  md: 'h-8.5 px-3.5 text-[13px] gap-2 rounded-lg',
  lg: 'h-10 px-5 text-[14px] gap-2 rounded-lg',
  icon: 'h-8 w-8 rounded-lg',
}

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: Size
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant = 'secondary', size = 'md', ...props },
  ref,
) {
  return (
    <button
      ref={ref}
      className={cn(
        'no-drag inline-flex shrink-0 items-center justify-center font-medium whitespace-nowrap',
        'transition-[background,color,opacity,box-shadow] duration-150',
        'disabled:pointer-events-none disabled:opacity-45',
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...props}
    />
  )
})
