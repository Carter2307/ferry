import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import type { CreateDatastore, DatastoreView } from '../types'

/** `GET /api/v1/datastores` */
export function useDatastores() {
  const refetchInterval = usePollInterval(5000)
  return useQuery({
    queryKey: keys.datastoreList(),
    queryFn: ({ signal }) => endpoints.listDatastores(signal),
    refetchInterval,
  })
}

/** `GET /api/v1/datastores/{ref}` — polls while `creating`. */
export function useDatastore(ref: string | undefined) {
  const qc = useQueryClient()
  const poll = usePollInterval(2000)
  return useQuery<DatastoreView, Error, DatastoreView, ReturnType<typeof keys.datastore>>({
    queryKey: keys.datastore(ref ?? ''),
    queryFn: ({ signal }) => endpoints.getDatastore(ref ?? '', signal),
    enabled: Boolean(ref),
    placeholderData: () =>
      qc.getQueryData<DatastoreView[]>(keys.datastoreList())?.find((d) => d.name === ref || d.id === ref),
    refetchInterval: (q) => (q.state.data?.status === 'creating' ? poll : false),
  })
}

/** `POST /api/v1/datastores` → 201 (status `creating`, then provisioned). */
export function useCreateDatastore() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateDatastore) => endpoints.createDatastore(body),
    onSuccess: (ds) => {
      qc.setQueryData(keys.datastore(ds.name), ds)
      void qc.invalidateQueries({ queryKey: keys.datastoreList() })
    },
  })
}

/** `DELETE /api/v1/datastores/{ref}?force=` (409 while referenced unless force). */
export function useDeleteDatastore() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ ref, force = false }: { ref: string; force?: boolean }) => endpoints.deleteDatastore(ref, force),
    onSuccess: (_v, { ref }) => {
      qc.setQueryData<DatastoreView[]>(keys.datastoreList(), (list) =>
        list?.filter((d) => d.name !== ref && d.id !== ref),
      )
      qc.removeQueries({ queryKey: keys.datastore(ref) })
      void qc.invalidateQueries({ queryKey: keys.datastores() })
    },
  })
}
