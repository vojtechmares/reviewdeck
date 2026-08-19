import { useState } from 'react'
import { Pencil, Plus, Trash2 } from 'lucide-react'
import { type ReviewWindow } from '@shared/types'
import { dayName, describeWindow, windowProblem } from '@shared/review-window'
import { useApp } from '@/hooks/useApp'
import { Button } from './ui/button'
import { Dialog } from './ui/dialog'
import { Input, Label } from './ui/input'

/** Monday first, because that is how a working week is read. */
const WEEK = [1, 2, 3, 4, 5, 6, 0]

const WEEKDAYS = [1, 2, 3, 4, 5]

function blankWindow(): ReviewWindow {
  return {
    id: crypto.randomUUID(),
    enabled: true,
    days: WEEKDAYS,
    start: '09:00',
    end: '09:30',
    minimum: 1,
  }
}

export function ScheduleDialog({
  open,
  onClose,
}: {
  open: boolean
  onClose: () => void
}): React.JSX.Element {
  const { settings, updateSettings } = useApp()
  const [editing, setEditing] = useState<ReviewWindow | null>(null)

  const windows = settings.reviewWindows
  const problem = editing ? windowProblem(editing) : null

  const save = (): void => {
    if (!editing || problem) return
    const next = windows.some((window) => window.id === editing.id)
      ? windows.map((window) => (window.id === editing.id ? editing : window))
      : [...windows, editing]
    void updateSettings({ reviewWindows: next })
    setEditing(null)
  }

  const remove = (id: string): void => {
    void updateSettings({ reviewWindows: windows.filter((window) => window.id !== id) })
  }

  const patch = (values: Partial<ReviewWindow>): void => {
    setEditing((current) => (current ? { ...current, ...values } : current))
  }

  const toggleDay = (day: number): void => {
    if (!editing) return
    patch({
      days: editing.days.includes(day)
        ? editing.days.filter((entry) => entry !== day)
        : [...editing.days, day],
    })
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Review schedule"
      description="While a window is open Reviewdeck may interrupt you. Outside every window it stays quiet and lets reviews collect. No windows means it notifies whenever it finds something."
      className="max-w-xl"
      footer={
        editing ? (
          <>
            {problem && <span className="mr-auto text-[11.5px] text-bad">{problem}</span>}
            <Button variant="ghost" onClick={() => setEditing(null)}>
              Cancel
            </Button>
            <Button variant="default" onClick={save} disabled={Boolean(problem)}>
              Save
            </Button>
          </>
        ) : (
          <Button variant="default" onClick={() => setEditing(blankWindow())}>
            <Plus className="size-3.5" />
            Add window
          </Button>
        )
      }
    >
      {!editing ? (
        <div className="flex flex-col gap-2">
          <ul className="flex flex-col gap-2">
            {windows.map((window) => (
              <li key={window.id} className="glass flex items-center gap-3 rounded-lg px-3 py-2.5">
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[13px] font-medium">{describeWindow(window)}</p>
                  {!window.enabled && (
                    <p className="text-[11.5px] text-muted-foreground">Turned off</p>
                  )}
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Edit ${describeWindow(window)}`}
                  onClick={() => setEditing(window)}
                >
                  <Pencil className="size-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${describeWindow(window)}`}
                  onClick={() => remove(window.id)}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </li>
            ))}
          </ul>

          {!windows.length && (
            <p className="py-8 text-center text-[13px] text-muted-foreground">
              No windows yet, so Reviewdeck notifies you whenever a poll finds something.
            </p>
          )}

          <p className="text-[11px] leading-relaxed text-muted-foreground">
            A window only fires while Reviewdeck is running. If you want the morning
            roll-up to be there before you are, turn on “Launch Reviewdeck at login” in
            Settings.
          </p>
        </div>
      ) : (
        <div className="flex flex-col gap-3.5">
          <div>
            <Label>Days</Label>
            <div className="flex gap-1.5">
              {WEEK.map((day) => (
                <button
                  key={day}
                  type="button"
                  aria-pressed={editing.days.includes(day)}
                  onClick={() => toggleDay(day)}
                  className={cnDay(editing.days.includes(day))}
                >
                  {dayName(day)}
                </button>
              ))}
            </div>
          </div>

          <div className="flex gap-3">
            <div className="flex-1">
              <Label htmlFor="window-start">From</Label>
              <Input
                id="window-start"
                type="time"
                value={editing.start}
                onChange={(event) => patch({ start: event.target.value })}
              />
            </div>
            <div className="flex-1">
              <Label htmlFor="window-end">Until</Label>
              <Input
                id="window-end"
                type="time"
                value={editing.end}
                onChange={(event) => patch({ end: event.target.value })}
              />
            </div>
          </div>
          <p className="-mt-2 text-[11px] text-muted-foreground">
            A window cannot cross midnight. For 22:00 to 02:00, make two.
          </p>

          <div>
            <Label htmlFor="window-minimum">Only if this many reviews are waiting</Label>
            <Input
              id="window-minimum"
              type="number"
              min={1}
              value={editing.minimum}
              onChange={(event) => patch({ minimum: Math.trunc(Number(event.target.value)) })}
            />
            <p className="mt-1 text-[11px] text-muted-foreground">
              Checked for as long as the window is open, not just as it opens - so a
              window that starts quiet still fires the moment the count is reached.
            </p>
          </div>

          <label className="flex cursor-pointer items-center gap-2 text-[12.5px]">
            <input
              type="checkbox"
              checked={editing.enabled}
              onChange={(event) => patch({ enabled: event.target.checked })}
            />
            Use this window
          </label>
        </div>
      )}
    </Dialog>
  )
}

function cnDay(active: boolean): string {
  return [
    'h-8 flex-1 rounded-lg border text-[12px] font-medium transition-colors',
    active
      ? 'border-border-strong bg-surface-strong'
      : 'border-border bg-transparent text-muted-foreground hover:bg-accent hover:text-foreground',
  ].join(' ')
}
