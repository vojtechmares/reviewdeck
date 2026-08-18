import type { ReviewdeckApi } from './index.ts'

declare global {
  interface Window {
    reviewdeck: ReviewdeckApi
  }
}

export {}
