import { describe, expect, it } from 'vitest'

import { invalidationsForChange } from './events'
import { keys } from './keys'

describe('invalidationsForChange', () => {
  it('maps service changes to the whole services subtree', () => {
    expect(invalidationsForChange({ kind: 'service', id: 'srv-1', service_id: null, action: 'updated' })).toEqual([
      keys.services(),
    ])
  })
  it('maps deploy changes to the deploy and the service views', () => {
    expect(invalidationsForChange({ kind: 'deploy', id: 'dep-1', service_id: 'srv-1', action: 'created' })).toEqual([
      keys.deploy('dep-1'),
      keys.services(),
    ])
  })
  it('maps job changes to the job and every jobs list', () => {
    expect(invalidationsForChange({ kind: 'job', id: 'job-1', service_id: 'srv-1', action: 'updated' })).toEqual([
      keys.job('job-1'),
      'service-jobs',
    ])
  })
  it('refreshes services when an env group is deleted', () => {
    expect(invalidationsForChange({ kind: 'env_group', id: 'eg-1', service_id: null, action: 'deleted' })).toEqual([
      keys.envGroups(),
      keys.services(),
    ])
    expect(invalidationsForChange({ kind: 'datastore', id: 'ds-1', service_id: null, action: 'updated' })).toEqual([
      keys.datastores(),
    ])
  })
})
