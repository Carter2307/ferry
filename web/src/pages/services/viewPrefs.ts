import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'

import { safeStorage } from '@/lib/storage'

import type { ServicesSort, ServicesView } from './lib'

interface ServicesViewPrefs {
  view: ServicesView
  sort: ServicesSort
  setView: (view: ServicesView) => void
  setSort: (sort: ServicesSort) => void
}

/**
 * Services list preferences (grid/list, sort), persisted per browser.
 * Local to this page until `useUi` grows a `servicesView` field.
 */
export const useServicesViewPrefs = create<ServicesViewPrefs>()(
  persist(
    (set) => ({
      view: 'grid',
      sort: 'name',
      setView: (view) => set({ view }),
      setSort: (sort) => set({ sort }),
    }),
    {
      name: 'ferry.services-view',
      version: 1,
      storage: createJSONStorage(() => ({
        getItem: (k) => safeStorage.get(k),
        setItem: (k, v) => safeStorage.set(k, v),
        removeItem: (k) => safeStorage.remove(k),
      })),
      partialize: (s) => ({ view: s.view, sort: s.sort }),
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<ServicesViewPrefs>
        return {
          ...current,
          view: p.view === 'list' || p.view === 'grid' ? p.view : current.view,
          sort: p.sort === 'last_deploy' || p.sort === 'name' ? p.sort : current.sort,
        }
      },
    },
  ),
)
