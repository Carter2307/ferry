import * as React from 'react'
import { useNavigate } from 'react-router'
import {
  Braces,
  ExternalLink,
  History,
  LayoutDashboard,
  ListChecks,
  LogOut,
  Monitor,
  Moon,
  PanelLeft,
  Plus,
  Rocket,
  RotateCw,
  ScrollText,
  Settings2,
  SlidersHorizontal,
  Sun,
} from 'lucide-react'
import { toast } from 'sonner'

import { DatastoreKindIcon, ServiceTypeIcon } from '@/components/patterns/icons'
import { SERVICE_STATE_TONE } from '@/components/patterns/status-tones'
import { StatusDot } from '@/components/patterns/StatusBadge'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command'
import { errorMessage } from '@/lib/api/client'
import {
  useDatastores,
  useEnvGroups,
  useRestartService,
  useService,
  useServices,
  useTriggerDeploy,
} from '@/lib/api/queries'
import { DATASTORE_KIND_LABELS, SERVICE_TYPE_LABELS } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import { useUi } from '@/stores/ui'

import { NAV_ITEMS } from './nav'
import { useRouteResource } from './useRouteResource'

/** ⌘K palette: jump to any service / datastore / env group / page, plus common actions. */
export function CommandMenu() {
  const open = useUi((s) => s.commandOpen)
  const setOpen = useUi((s) => s.setCommandOpen)
  const setTheme = useUi((s) => s.setTheme)
  const toggleRail = useUi((s) => s.toggleRail)
  const signOut = useAuth((s) => s.signOut)
  const navigate = useNavigate()
  const resource = useRouteResource()
  const current = resource.kind === 'service' ? resource.name : undefined

  const services = useServices()
  const datastores = useDatastores()
  const groups = useEnvGroups()
  const { data: service } = useService(current)
  const deploy = useTriggerDeploy(current ?? '')
  const restart = useRestartService(current ?? '')

  const [search, setSearch] = React.useState('')
  const searching = search.trim() !== ''
  // Every opening starts from an empty query.
  const [wasOpen, setWasOpen] = React.useState(open)
  if (open !== wasOpen) {
    setWasOpen(open)
    if (open) setSearch('')
  }

  const run = (fn: () => void) => {
    setOpen(false)
    fn()
  }
  const go = (to: string) => run(() => void navigate(to))

  const svcPath = (name: string, sub = '') => `/services/${encodeURIComponent(name)}${sub}`

  // Side-effect actions (deploy, restart) come last so the pre-highlighted item is harmless
  // (⌘K then Enter). Once the user types, they move first: typed intent ("restart") should win
  // over weak fuzzy matches, and cmdk only re-sorts items inside a group, not the groups.
  const actionsGroup = current ? (
    <>
      <CommandSeparator />
      <CommandGroup heading={`Actions · ${current}`}>
        <CommandItem
          value={`Manual deploy ${current}`}
          keywords={['deploy', 'build', 'release']}
          disabled={deploy.isPending}
          onSelect={() =>
            run(() =>
              deploy.mutate(
                {},
                {
                  onSuccess: () => toast.success(`Deploy of ${current} queued`),
                  onError: (e) => toast.error('Deploy failed to start', { description: errorMessage(e) }),
                },
              ),
            )
          }
        >
          <Rocket /> Manual deploy
        </CommandItem>
        <CommandItem
          value={`Restart service ${current}`}
          keywords={['restart', 'reboot']}
          disabled={restart.isPending || service?.state === 'suspended'}
          onSelect={() =>
            run(() =>
              restart.mutate(undefined, {
                onSuccess: () => toast.success(`Restarting ${current}`),
                onError: (e) => toast.error('Restart failed', { description: errorMessage(e) }),
              }),
            )
          }
        >
          <RotateCw /> Restart service
        </CommandItem>
      </CommandGroup>
    </>
  ) : null

  return (
    <CommandDialog open={open} onOpenChange={setOpen} title="Search Ferry" description="Jump to a resource or run an action">
      <CommandInput placeholder="Search services, datastores, pages, actions…" value={search} onValueChange={setSearch} />
      <CommandList>
        <CommandEmpty>No results found.</CommandEmpty>

        {searching && actionsGroup}

        {/* Navigation first: the pre-highlighted item must be harmless (⌘K then Enter).
            Side-effect actions (deploy, restart) come last, after the pages. */}
        {current && (
          <>
            <CommandGroup heading={`Service · ${current}`}>
              <CommandItem value={`Overview ${current}`}
                keywords={['overview', 'home', 'status']} onSelect={() => go(svcPath(current))}>
                <LayoutDashboard /> Overview
              </CommandItem>
              <CommandItem value={`Deploys ${current}`}
                keywords={['deploys', 'history', 'builds']} onSelect={() => go(svcPath(current, '/deploys'))}>
                <History /> Deploys
              </CommandItem>
              <CommandItem value={`View logs ${current}`}
                keywords={['logs', 'runtime', 'output']} onSelect={() => go(svcPath(current, '/logs'))}>
                <ScrollText /> View logs
              </CommandItem>
              <CommandItem value={`Jobs ${current}`}
                keywords={['jobs', 'run', 'one-off', 'migrate']} onSelect={() => go(svcPath(current, '/jobs'))}>
                <ListChecks /> Jobs
              </CommandItem>
              <CommandItem value={`Environment ${current}`}
                keywords={['env', 'variables', 'secrets']} onSelect={() => go(svcPath(current, '/environment'))}>
                <SlidersHorizontal /> Environment
              </CommandItem>
              <CommandItem value={`Settings ${current}`}
                keywords={['settings', 'domains', 'build']} onSelect={() => go(svcPath(current, '/settings'))}>
                <Settings2 /> Settings
              </CommandItem>
              {service?.url && (
                <CommandItem
                  value={`Open URL ${current}`}
                  keywords={['open', 'url', 'visit']}
                  onSelect={() => run(() => window.open(service.url ?? '', '_blank', 'noopener,noreferrer'))}
                >
                  <ExternalLink /> Open {service.url.replace(/^https?:\/\//, '')}
                </CommandItem>
              )}
            </CommandGroup>
            <CommandSeparator />
          </>
        )}

        {(services.data?.length ?? 0) > 0 && (
          <CommandGroup heading="Services">
            {services.data?.map((s) => (
              <CommandItem key={s.id} value={s.name}
                keywords={['service', SERVICE_TYPE_LABELS[s.type]]} onSelect={() => go(svcPath(s.name))}>
                <ServiceTypeIcon type={s.type} />
                <span className="truncate">{s.name}</span>
                <span className="ml-auto flex items-center gap-2 text-[12px] text-foreground-lighter">
                  {SERVICE_TYPE_LABELS[s.type]}
                  <StatusDot tone={SERVICE_STATE_TONE[s.state].tone} />
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}

        {(datastores.data?.length ?? 0) > 0 && (
          <CommandGroup heading="Datastores">
            {datastores.data?.map((d) => (
              <CommandItem key={d.id} value={`datastore ${d.name}`}
                keywords={[d.name, d.kind, 'database']} onSelect={() => go(`/datastores/${encodeURIComponent(d.name)}`)}>
                <DatastoreKindIcon kind={d.kind} />
                <span className="truncate">{d.name}</span>
                <span className="ml-auto text-[12px] text-foreground-lighter">
                  {DATASTORE_KIND_LABELS[d.kind]} {d.version}
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}

        {(groups.data?.length ?? 0) > 0 && (
          <CommandGroup heading="Env groups">
            {groups.data?.map((g) => (
              <CommandItem key={g.id} value={`env group ${g.name}`}
                keywords={[g.name, 'environment']} onSelect={() => go(`/env-groups/${encodeURIComponent(g.name)}`)}>
                <Braces />
                <span className="truncate">{g.name}</span>
                <span className="ml-auto text-[12px] text-foreground-lighter">
                  {g.vars.length} var{g.vars.length === 1 ? '' : 's'}
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}

        <CommandSeparator />
        <CommandGroup heading="Pages">
          {NAV_ITEMS.flat().map((item) => (
            <CommandItem key={item.to} value={`Go to ${item.label}`}
              keywords={[item.label, item.hint]} onSelect={() => go(item.to)}>
              <item.icon />
              {item.label}
              <span className="ml-auto truncate text-[12px] text-foreground-lighter">{item.hint}</span>
            </CommandItem>
          ))}
          <CommandItem value="New service"
            keywords={['create', 'add']} onSelect={() => go('/services/new')}>
            <Plus /> New service
          </CommandItem>
        </CommandGroup>

        {!searching && actionsGroup}

        <CommandSeparator />
        <CommandGroup heading="Preferences">
          <CommandItem value="Light theme"
            keywords={['theme', 'appearance', 'light']} onSelect={() => run(() => setTheme('light'))}>
            <Sun /> Light theme
          </CommandItem>
          <CommandItem value="Dark theme"
            keywords={['theme', 'appearance', 'dark']} onSelect={() => run(() => setTheme('dark'))}>
            <Moon /> Dark theme
          </CommandItem>
          <CommandItem value="System theme"
            keywords={['theme', 'appearance', 'auto']} onSelect={() => run(() => setTheme('system'))}>
            <Monitor /> System theme
          </CommandItem>
          <CommandItem value="Toggle sidebar labels"
            keywords={['sidebar', 'menu', 'collapse', 'expand']} onSelect={() => run(toggleRail)}>
            <PanelLeft /> Toggle sidebar labels
          </CommandItem>
          <CommandItem value="Sign out"
            keywords={['logout', 'log out']} onSelect={() => run(signOut)}>
            <LogOut /> Sign out
          </CommandItem>
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
