import { Content, Portal, Root, Trigger } from '@radix-ui/react-popover'
import type { ComponentPropsWithoutRef } from 'react'
import { cn } from '@/lib/utils'

/**
 * A floating panel anchored to the control that opens it.
 *
 * The one primitive here that is not hand-rolled, and for a reason the hand-rolled
 * `Tooltip` shows: positioned with plain CSS, a hint lives inside its trigger's
 * stacking and overflow context and cannot get out of it - so a panel opened from a
 * header renders underneath the content below that header. The content is portalled
 * to the document body instead, which is what actually fixes the layering; a
 * z-index would not. The theme class sits on the document element, so a portalled
 * panel is still the right theme.
 */
export const Popover = Root
export const PopoverTrigger = Trigger

export function PopoverContent({
  className,
  align = 'start',
  sideOffset = 6,
  ...props
}: ComponentPropsWithoutRef<typeof Content>): React.JSX.Element {
  return (
    <Portal>
      <Content
        align={align}
        sideOffset={sideOffset}
        collisionPadding={12}
        className={cn(
          'glass-overlay rise z-50 overflow-hidden rounded-lg outline-none',
          // Radix measures the trigger and the viewport for us; spending it on a
          // panel that can scroll keeps a long list off the edge of the window.
          'max-h-(--radix-popover-content-available-height) overflow-y-auto',
          className,
        )}
        {...props}
      />
    </Portal>
  )
}
