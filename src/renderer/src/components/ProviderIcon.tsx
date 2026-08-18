import type { ProviderKind } from '@shared/types'
import { cn } from '@/lib/utils'

/**
 * Inline provider marks. Bundling the paths avoids any network fetch and keeps
 * the strict CSP happy - remote images from providers are avatars only.
 */
const PATHS: Record<ProviderKind, string> = {
  github:
    'M12 .5C5.37.5 0 5.87 0 12.5c0 5.3 3.44 9.8 8.2 11.39.6.11.82-.26.82-.58l-.01-2.03c-3.34.73-4.04-1.61-4.04-1.61-.55-1.39-1.34-1.76-1.34-1.76-1.08-.75.09-.73.09-.73 1.2.08 1.83 1.24 1.83 1.24 1.07 1.83 2.81 1.3 3.5.99.11-.78.42-1.3.76-1.6-2.67-.3-5.47-1.34-5.47-5.96 0-1.32.47-2.39 1.24-3.23-.13-.3-.54-1.53.11-3.18 0 0 1.01-.32 3.3 1.23a11.5 11.5 0 0 1 6.01 0c2.29-1.55 3.3-1.23 3.3-1.23.65 1.65.24 2.88.12 3.18.77.84 1.23 1.91 1.23 3.23 0 4.63-2.8 5.65-5.48 5.95.43.37.81 1.1.81 2.22l-.01 3.29c0 .32.21.7.82.58A12 12 0 0 0 24 12.5C24 5.87 18.63.5 12 .5Z',
  gitlab:
    'M12 23.2 16.42 9.6H7.58L12 23.2ZM3.16 9.6 1.5 14.72c-.15.47.02.98.42 1.27L12 23.2 3.16 9.6ZM3.16 9.6h4.42L5.68 3.77c-.1-.3-.52-.3-.62 0L3.16 9.6ZM20.84 9.6l1.66 5.12c.15.47-.02.98-.42 1.27L12 23.2 20.84 9.6ZM20.84 9.6h-4.42l1.9-5.83c.1-.3.52-.3.62 0L20.84 9.6Z',
  forgejo:
    'M17.4 2.4a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4Zm0 2.2a2 2 0 1 1 0 4 2 2 0 0 1 0-4ZM6.6 13.2a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4Zm0 2.2a2 2 0 1 1 0 4 2 2 0 0 1 0-4ZM6.6 2.4a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4Zm0 2.2a2 2 0 1 1 0 4 2 2 0 0 1 0-4ZM5.5 10.2h2.2v4.2H5.5v-4.2Zm2.9 1.5h4.3a2 2 0 0 0 2-2V8.4h2.2v1.3a4.2 4.2 0 0 1-4.2 4.2H8.4v-2.2Z',
  bitbucket:
    'M1.6 2.4a.8.8 0 0 0-.79.93l3.2 17.1a.8.8 0 0 0 .79.67h14.4a.8.8 0 0 0 .79-.67l3.2-17.1a.8.8 0 0 0-.79-.93H1.6Zm12.9 12.2h-5L8.2 8.9h7.6l-1.3 5.7Z',
}

export function ProviderIcon({
  kind,
  className,
}: {
  kind: ProviderKind
  className?: string
}): React.JSX.Element {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      className={cn('size-3.5 shrink-0 fill-current', className)}
    >
      <path d={PATHS[kind]} />
    </svg>
  )
}
