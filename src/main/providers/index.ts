import type { ProviderKind } from '@shared/types.ts'
import type { Provider } from './types.ts'
import { github } from './github.ts'
import { gitlab } from './gitlab.ts'
import { forgejo } from './forgejo.ts'
import { bitbucket } from './bitbucket.ts'

const REGISTRY: Record<ProviderKind, Provider> = { github, gitlab, forgejo, bitbucket }

export function providerFor(kind: ProviderKind): Provider {
  const provider = REGISTRY[kind]
  if (!provider) throw new Error(`Unsupported provider: ${kind}`)
  return provider
}

export type { Provider, Session } from './types.ts'
