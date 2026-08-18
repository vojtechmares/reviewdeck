import { useEffect, useState } from 'react'
import { DEFAULT_AGENT_COMMAND, type Settings } from '@shared/types'
import { useApp } from '@/hooks/useApp'
import { Dialog } from './ui/dialog'
import { Input, Label, Select } from './ui/input'
import { Button } from './ui/button'

export function SettingsDialog({
  open,
  onClose,
}: {
  open: boolean
  onClose: () => void
}): React.JSX.Element {
  const { settings, updateSettings } = useApp()
  const [info, setInfo] = useState<{ version: string; electron: string } | null>(null)

  useEffect(() => {
    if (open) void window.reviewdeck.app.info().then(setInfo)
  }, [open])

  const set = <K extends keyof Settings>(key: K, value: Settings[K]): void => {
    void updateSettings({ [key]: value } as Partial<Settings>)
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Settings"
      className="max-w-md"
      footer={
        <>
          {info && (
            <span className="mr-auto text-[11px] text-muted-foreground">
              Reviewdeck {info.version} · Electron {info.electron}
            </span>
          )}
          <Button variant="default" onClick={onClose}>
            Done
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Section title="Syncing">
          <Field label="Check for new reviews every">
            <Select
              value={String(settings.pollInterval)}
              onChange={(event) => set('pollInterval', Number(event.target.value))}
            >
              <option value="60">1 minute</option>
              <option value="180">3 minutes</option>
              <option value="300">5 minutes</option>
              <option value="900">15 minutes</option>
              <option value="1800">30 minutes</option>
            </Select>
          </Field>
          <Field label="Re-poll running checks every">
            <Select
              value={String(settings.checkPollInterval)}
              onChange={(event) => set('checkPollInterval', Number(event.target.value))}
            >
              <option value="20">20 seconds</option>
              <option value="45">45 seconds</option>
              <option value="90">90 seconds</option>
              <option value="180">3 minutes</option>
            </Select>
          </Field>
        </Section>

        <Section title="Notifications">
          <Toggle
            label="Notify me about new review requests"
            checked={settings.notificationsEnabled}
            onChange={(value) => set('notificationsEnabled', value)}
          />
          <Toggle
            label="Play a sound"
            checked={settings.playSound}
            disabled={!settings.notificationsEnabled}
            onChange={(value) => set('playSound', value)}
          />
        </Section>

        <Section title="Appearance">
          <Field label="Theme">
            <Select
              value={settings.theme}
              onChange={(event) => set('theme', event.target.value as Settings['theme'])}
            >
              <option value="system">Match macOS</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </Select>
          </Field>
          <Field label="Diff layout">
            <Select
              value={settings.diffView}
              onChange={(event) => set('diffView', event.target.value as Settings['diffView'])}
            >
              <option value="split">Side by side</option>
              <option value="unified">Unified</option>
            </Select>
          </Field>
          <Toggle
            label="Hide pull requests I already approved"
            checked={settings.hideApproved}
            onChange={(value) => set('hideApproved', value)}
          />
        </Section>

        <Section title="Agent handoff">
          <Field label="Command to copy">
            <Input
              value={settings.agentCommand}
              placeholder={DEFAULT_AGENT_COMMAND}
              spellCheck={false}
              onChange={(event) => set('agentCommand', event.target.value)}
            />
          </Field>
          <p className="text-[11px] text-muted-foreground">
            Used by “Copy Claude prompt”. A shell alias works, because it is your own shell
            that runs it - Reviewdeck only copies the text. Any account can override this.
          </p>
        </Section>

        <Section title="System">
          <Toggle
            label="Launch Reviewdeck at login"
            checked={settings.launchAtLogin}
            onChange={(value) => set('launchAtLogin', value)}
          />
        </Section>
      </div>
    </Dialog>
  )
}

function Section({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <section>
      <h3 className="mb-2 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
        {title}
      </h3>
      <div className="glass flex flex-col gap-2.5 rounded-lg p-3">{children}</div>
    </section>
  )
}

function Field({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex items-center gap-3">
      <Label className="mb-0 flex-1 !text-[12.5px] !text-foreground">{label}</Label>
      <div className="w-40 shrink-0">{children}</div>
    </div>
  )
}

/** A macOS-style switch; a checkbox underneath keeps it keyboard accessible. */
function Toggle({
  label,
  checked,
  onChange,
  disabled,
}: {
  label: string
  checked: boolean
  onChange: (value: boolean) => void
  disabled?: boolean
}): React.JSX.Element {
  return (
    <label
      className={`flex items-center gap-3 ${disabled ? 'opacity-45' : 'cursor-pointer'}`}
    >
      <span className="flex-1 text-[12.5px]">{label}</span>
      <span className="relative inline-flex shrink-0">
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked)}
          className="peer size-0 opacity-0"
        />
        <span
          aria-hidden="true"
          className={`h-5 w-8.5 rounded-full border border-border transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-ring ${
            checked ? 'bg-ok' : 'bg-muted'
          }`}
        />
        <span
          aria-hidden="true"
          className={`absolute top-0.5 size-4 rounded-full bg-white shadow-sm transition-all ${
            checked ? 'left-4' : 'left-0.5'
          }`}
        />
      </span>
    </label>
  )
}
