import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react'
import { AlertTriangle, Check, Info, X } from 'lucide-react'
import { cn } from '@/lib/utils'

type ToastTone = 'ok' | 'bad' | 'info'

interface Toast {
  id: number
  tone: ToastTone
  message: string
}

const ToastContext = createContext<(tone: ToastTone, message: string) => void>(() => {})

export function useToast(): {
  ok: (message: string) => void
  bad: (message: string) => void
  info: (message: string) => void
} {
  const push = useContext(ToastContext)
  return useMemo(
    () => ({
      ok: (message: string) => push('ok', message),
      bad: (message: string) => push('bad', message),
      info: (message: string) => push('info', message),
    }),
    [push],
  )
}

const ICONS = { ok: Check, bad: AlertTriangle, info: Info }
const TONES: Record<ToastTone, string> = {
  ok: 'text-ok',
  bad: 'text-bad',
  info: 'text-info',
}

export function ToastProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const [toasts, setToasts] = useState<Toast[]>([])

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id))
  }, [])

  const push = useCallback(
    (tone: ToastTone, message: string) => {
      const id = Date.now() + Math.random()
      setToasts((current) => [...current.slice(-3), { id, tone, message }])
      // Errors linger; confirmations get out of the way quickly.
      setTimeout(() => dismiss(id), tone === 'bad' ? 8000 : 3500)
    },
    [dismiss],
  )

  return (
    <ToastContext.Provider value={push}>
      {children}
      <div className="pointer-events-none fixed right-4 bottom-4 z-100 flex w-88 flex-col gap-2">
        {toasts.map((toast) => {
          const Icon = ICONS[toast.tone]
          return (
            <div
              key={toast.id}
              role="status"
              className="glass-overlay rise pointer-events-auto flex items-start gap-2.5 rounded-lg px-3 py-2.5"
            >
              <Icon className={cn('mt-px size-4 shrink-0', TONES[toast.tone])} />
              <p className="min-w-0 flex-1 text-[12.5px] leading-snug break-words">{toast.message}</p>
              <button
                onClick={() => dismiss(toast.id)}
                aria-label="Dismiss"
                className="text-muted-foreground transition-colors hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            </div>
          )
        })}
      </div>
    </ToastContext.Provider>
  )
}
