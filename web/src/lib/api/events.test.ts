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
  it('maps git connection changes to the accounts and everything read through them', () => {
    for (const action of ['created', 'updated', 'deleted'] as const) {
      expect(invalidationsForChange({ kind: 'git_connection', id: 'git-1', service_id: null, action })).toEqual([
        keys.git(),
      ])
    }
    // the prefix covers the accounts, their repositories and the branches of any repository
    for (const key of [keys.gitConnections(), keys.gitRepositories('git-1'), keys.gitBranches('https://x/a/b')]) {
      expect(key[0]).toBe(keys.git()[0])
    }
  })
  it('maps domain changes to the domains, the server info and every service', () => {
    // a domain that starts or stops being served changes the hosts and the URL of every service
    expect(invalidationsForChange({ kind: 'domain', id: 'dom-1', service_id: null, action: 'updated' })).toEqual([
      keys.domains(),
      keys.info(),
      keys.services(),
    ])
    for (const key of [keys.domainList(), keys.certificates()]) {
      expect(key[0]).toBe(keys.domains()[0])
    }
  })
})
