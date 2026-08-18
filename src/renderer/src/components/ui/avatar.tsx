import { useState } from 'react'
import { cn } from '@/lib/utils'
import { initials } from '@/lib/utils'

interface AvatarProps {
  src?: string
  name: string
  className?: string
}

/** Falls back to initials, which matters because self-hosted avatars often 404. */
export function Avatar({ src, name, className }: AvatarProps): React.JSX.Element {
  const [broken, setBroken] = useState(false)
  const showImage = src && !broken

  return (
    <span
      className={cn(
        'inline-flex size-6 shrink-0 items-center justify-center overflow-hidden rounded-full',
        'border border-border bg-muted text-[9.5px] font-semibold text-muted-foreground select-none',
        className,
      )}
      title={name}
    >
      {showImage ? (
        <img
          src={src}
          alt=""
          className="size-full object-cover"
          onError={() => setBroken(true)}
          draggable={false}
        />
      ) : (
        initials(name)
      )}
    </span>
  )
}
