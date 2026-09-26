import { useMutation, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { keys } from '../keys'
import type { ApplyBlueprint } from '../types'

/** `POST /api/v1/blueprints/apply` — `mutate({yaml, dry_run})` → BlueprintResult. */
export function useApplyBlueprint() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: ApplyBlueprint) => endpoints.applyBlueprint(body),
    onSuccess: (result) => {
      if (result.dry_run) return
      void qc.invalidateQueries({ queryKey: keys.services() })
      void qc.invalidateQueries({ queryKey: keys.datastores() })
      void qc.invalidateQueries({ queryKey: keys.envGroups() })
    },
  })
}
