import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs))
}

/** "3 minutes ago", "2 days ago" - short enough for a dense list. */
export function relativeTime(iso: string): string {
  const then = new Date(iso).getTime()
  if (Number.isNaN(then)) return ''
  const seconds = Math.round((Date.now() - then) / 1000)
  if (seconds < 45) return 'just now'
  const units: [number, Intl.RelativeTimeFormatUnit][] = [
    [60, 'second'],
    [3600, 'minute'],
    [86400, 'hour'],
    [604800, 'day'],
    [2629800, 'week'],
    [31557600, 'month'],
    [Infinity, 'year'],
  ]
  const divisors = [1, 60, 3600, 86400, 604800, 2629800, 31557600]
  const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto', style: 'narrow' })
  for (let i = 0; i < units.length; i++) {
    if (seconds < units[i][0]) {
      return formatter.format(-Math.round(seconds / divisors[i]), units[i][1])
    }
  }
  return ''
}

export function initials(name: string): string {
  const cleaned = name.replace(/[^a-zA-Z0-9 ]/g, ' ').trim()
  if (!cleaned) return '?'
  const parts = cleaned.split(/\s+/)
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
}

export function formatCount(value: number | undefined): string {
  if (value === undefined) return '-'
  if (value < 1000) return String(value)
  return `${(value / 1000).toFixed(1)}k`
}
