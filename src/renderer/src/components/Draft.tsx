import { useState } from 'react'
import { Pencil, Trash2 } from 'lucide-react'
import type { DraftComment } from '@shared/types'
import { Badge } from './ui/badge'
import { Button } from './ui/button'
import { Textarea } from './ui/input'
import { Markdown } from './Markdown'

export interface DraftActions {
  onEdit: (id: string, body: string) => Promise<void>
  onDelete: (id: string) => Promise<void>
}

/**
 * A remark written but not yet sent.
 *
 * Dashed rather than solid, and badged: it has to be obvious at a glance which
 * remarks are still yours to change and which the author has already been told
 * about, because they sit side by side on the same line.
 */
export function DraftCard({
  draft,
  onEdit,
  onDelete,
}: { draft: DraftComment } & DraftActions): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [body, setBody] = useState(draft.body)
  const [busy, setBusy] = useState(false)

  const run = async (action: () => Promise<void>): Promise<void> => {
    if (busy) return
    setBusy(true)
    try {
      await action()
      setEditing(false)
    } catch {
      /* the caller has already surfaced it; keep what was typed */
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="m-1.5 rounded-md border border-dashed border-info/50 bg-info-soft px-2.5 py-2 font-sans">
      <div className="mb-1 flex items-center gap-1.5">
        <Badge tone="info">Pending</Badge>
        {!editing && (
          <div className="ml-auto flex items-center gap-0.5">
            <Button
              variant="ghost"
              size="icon"
              aria-label="Edit this draft"
              disabled={busy}
              onClick={() => {
                setBody(draft.body)
                setEditing(true)
              }}
            >
              <Pencil className="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              aria-label="Delete this draft"
              disabled={busy}
              onClick={() => void run(() => onDelete(draft.id))}
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>
        )}
      </div>

      {editing ? (
        <>
          <Textarea
            autoFocus
            rows={3}
            value={body}
            onChange={(event) => setBody(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') setEditing(false)
              if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && body.trim()) {
                void run(() => onEdit(draft.id, body.trim()))
              }
            }}
          />
          <div className="mt-1.5 flex items-center justify-end gap-1.5">
            <span className="mr-auto text-[10.5px] text-muted-foreground">⌘↵ to save</span>
            <Button size="sm" variant="ghost" onClick={() => setEditing(false)} disabled={busy}>
              Cancel
            </Button>
            <Button
              size="sm"
              variant="default"
              disabled={!body.trim() || busy}
              onClick={() => void run(() => onEdit(draft.id, body.trim()))}
            >
              Save
            </Button>
          </div>
        </>
      ) : (
        <Markdown compact>{draft.body}</Markdown>
      )}
    </article>
  )
}
