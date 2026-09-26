import * as React from 'react'
import { Link, useLocation, useNavigate } from 'react-router'
import {
  BookOpen,
  Boxes,
  Braces,
  ChevronsUpDown,
  CircleHelp,
  KeyRound,
  LogOut,
  Menu,
  Monitor,
  Moon,
  Plus,
  Search,
  Server,
  Sun,
} from 'lucide-react'

import { Kbd } from '@/components/patterns/Kbd'
import { DatastoreKindIcon, FerryLogo, ServiceTypeIcon } from '@/components/patterns/icons'
import { DATASTORE_STATUS_TONE, SERVICE_STATE_TONE } from '@/components/patterns/status-tones'
import { DatastoreStatusBadge, ServiceStatePill, StatusDot } from '@/components/patterns/StatusBadge'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { Hint } from '@/components/ui/tooltip'
import { useLive } from '@/lib/api/events'
import { useDatastores, useEnvGroups, useServerInfo, useService, useServices } from '@/lib/api/queries'
import { DATASTORE_KIND_LABELS, SERVICE_TYPE_LABELS } from '@/lib/format'
import { isMac } from '@/lib/platform'
import { cn } from '@/lib/utils'
import { useAuth } from '@/stores/auth'
import { useUi, type ThemePreference } from '@/stores/ui'

import { ConnectPopover } from './ConnectPopover'
import { DOCS_URL } from './nav'
import { ResourceSwitcher } from './ResourceSwitcher'
import { useRouteResource } from './useRouteResource'

function Slash() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" className="size-5 shrink-0 text-border-stronger" fill="none">
      <path d="M16 3.5 8 20.5" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
    </svg>
  )
}

const iconButton =
  'size-8 rounded-full border-border-strong bg-transparent text-foreground-lighter hover:text-foreground [&_svg:not([class*=size-])]:size-4'

/** Server segment: host + version pill ⇕ → server details dropdown. */
function ServerSegment() {
  const { data: info, isLoading } = useServerInfo()
  const mode = useLive((s) => s.mode)
  const navigate = useNavigate()
  const live = mode === 'live'
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Server"
          className="group inline-flex h-8 min-w-0 cursor-pointer items-center gap-2 rounded-md px-1.5 text-sm text-foreground outline-none transition-colors hover:bg-surface-200 focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:bg-surface-200"
        >
          <Server className="size-4 shrink-0 text-foreground-lighter" aria-hidden="true" />
          {isLoading ? (
            <Skeleton className="h-4 w-20" />
          ) : (
            <span className="max-w-[160px] truncate">{info?.base_domain || 'Ferry'}</span>
          )}
          {info && (
            <Badge font="mono" case="normal" className="hidden xl:inline-flex">
              v{info.version}
            </Badge>
          )}
          <ChevronsUpDown className="size-3.5 shrink-0 text-foreground-lighter" aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        <DropdownMenuLabel>Ferry server</DropdownMenuLabel>
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 px-2 pt-1 pb-2 text-[13px]">
          <dt className="text-foreground-lighter">Version</dt>
          <dd className="truncate text-right font-mono text-foreground">{info ? `v${info.version}` : '—'}</dd>
          <dt className="text-foreground-lighter">Docker</dt>
          <dd className="truncate text-right font-mono text-foreground">{info?.docker_version ?? 'unreachable'}</dd>
          <dt className="text-foreground-lighter">Proxy</dt>
          <dd className="truncate text-right font-mono text-foreground">{info?.proxy_url ?? '—'}</dd>
          <dt className="text-foreground-lighter">Updates</dt>
          <dd className="flex items-center justify-end gap-1.5 text-foreground">
            <StatusDot tone={live ? 'success' : 'neutral'} pulse={live} />
            {live ? 'Live' : mode === 'off' ? 'Off' : 'Polling'}
          </dd>
        </dl>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void navigate('/server')}>
          <Server />
          Server details
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

const SERVICE_SECTIONS = new Set(['deploys', 'logs', 'jobs', 'environment', 'settings'])

function ServiceSegment({ name }: { name: string }) {
  const { data: services, isLoading } = useServices()
  const { data: service } = useService(name)
  // Switching service keeps the current section (…/logs → other/logs).
  const { pathname } = useLocation()
  const section = pathname.split('/')[3] ?? ''
  const suffix = SERVICE_SECTIONS.has(section) ? `/${section}` : ''
  return (
    <>
      <Slash />
      <ResourceSwitcher
        label={`Service ${name}, switch service`}
        current={service?.name ?? name}
        placeholder="Find service…"
        loading={isLoading}
        items={(services ?? []).map((s) => ({
          value: s.name,
          label: s.name,
          to: `/services/${encodeURIComponent(s.name)}${suffix}`,
          icon: <ServiceTypeIcon type={s.type} className="text-foreground-lighter" />,
          meta: <StatusDot tone={SERVICE_STATE_TONE[s.state].tone} pulse={SERVICE_STATE_TONE[s.state].pulse} />,
        }))}
        footer={[
          { label: 'All services', to: '/services', icon: <Boxes /> },
          { label: 'New service', to: '/services/new', icon: <Plus /> },
        ]}
      >
        {service ? (
          <ServiceTypeIcon type={service.type} className="size-4 shrink-0 text-foreground-lighter" />
        ) : (
          <Boxes className="size-4 shrink-0 text-foreground-lighter" aria-hidden="true" />
        )}
        <span className="max-w-[180px] truncate">{name}</span>
      </ResourceSwitcher>
      {service && (
        <span className="hidden items-center gap-1.5 xl:flex">
          <Badge variant="outline">{SERVICE_TYPE_LABELS[service.type]}</Badge>
          <ServiceStatePill state={service.state} />
        </span>
      )}
    </>
  )
}

function DatastoreSegment({ name }: { name: string }) {
  const { data: datastores, isLoading } = useDatastores()
  const ds = datastores?.find((d) => d.name === name || d.id === name)
  return (
    <>
      <Slash />
      <ResourceSwitcher
        label={`Datastore ${name}, switch datastore`}
        current={ds?.name ?? name}
        placeholder="Find datastore…"
        loading={isLoading}
        items={(datastores ?? []).map((d) => ({
          value: d.name,
          label: d.name,
          to: `/datastores/${encodeURIComponent(d.name)}`,
          icon: <DatastoreKindIcon kind={d.kind} className="text-foreground-lighter" />,
          meta: <StatusDot tone={DATASTORE_STATUS_TONE[d.status].tone} pulse={DATASTORE_STATUS_TONE[d.status].pulse} />,
        }))}
        footer={[{ label: 'All datastores', to: '/datastores', icon: <Boxes /> }]}
      >
        {ds ? <DatastoreKindIcon kind={ds.kind} className="size-4 shrink-0 text-foreground-lighter" /> : null}
        <span className="max-w-[180px] truncate">{name}</span>
      </ResourceSwitcher>
      {ds && (
        <span className="hidden items-center gap-1.5 xl:flex">
          <Badge variant="outline">
            {DATASTORE_KIND_LABELS[ds.kind]} {ds.version}
          </Badge>
          <DatastoreStatusBadge status={ds.status} />
        </span>
      )}
    </>
  )
}

function EnvGroupSegment({ name }: { name: string }) {
  const { data: groups, isLoading } = useEnvGroups()
  return (
    <>
      <Slash />
      <ResourceSwitcher
        label={`Env group ${name}, switch env group`}
        current={name}
        placeholder="Find env group…"
        loading={isLoading}
        items={(groups ?? []).map((g) => ({
          value: g.name,
          label: g.name,
          to: `/env-groups/${encodeURIComponent(g.name)}`,
          icon: <Braces className="text-foreground-lighter" />,
          meta: <span className="font-mono text-[11px] text-foreground-lighter">{g.vars.length}</span>,
        }))}
        footer={[{ label: 'All env groups', to: '/env-groups', icon: <Braces /> }]}
      >
        <Braces className="size-4 shrink-0 text-foreground-lighter" aria-hidden="true" />
        <span className="max-w-[180px] truncate">{name}</span>
      </ResourceSwitcher>
    </>
  )
}

const THEME_ICON: Record<ThemePreference, React.ComponentType<{ className?: string }>> = {
  light: Sun,
  dark: Moon,
  system: Monitor,
}

export function ThemeMenu({ className }: { className?: string }) {
  const theme = useUi((s) => s.theme)
  const setTheme = useUi((s) => s.setTheme)
  const Icon = THEME_ICON[theme]
  return (
    <DropdownMenu>
      <Hint label="Theme">
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="icon-lg" className={cn(iconButton, className)} aria-label={`Theme: ${theme}`}>
            <Icon />
          </Button>
        </DropdownMenuTrigger>
      </Hint>
      <DropdownMenuContent align="end" className="w-40">
        <DropdownMenuLabel>Theme</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={theme} onValueChange={(v) => setTheme(v as ThemePreference)}>
          <DropdownMenuRadioItem value="light">
            <Sun /> Light
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="dark">
            <Moon /> Dark
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="system">
            <Monitor /> System
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function AccountMenu() {
  const token = useAuth((s) => s.token) ?? ''
  const signOut = useAuth((s) => s.signOut)
  const navigate = useNavigate()
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Account"
          className="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full border border-primary/30 bg-primary-soft text-primary outline-none transition-colors hover:border-primary/60 focus-visible:ring-2 focus-visible:ring-ring"
        >
          <KeyRound className="size-3.5" aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60">
        <div className="flex flex-col gap-0.5 px-2 py-2">
          <span className="text-[13px] font-medium text-foreground">Signed in with an API token</span>
          <span className="font-mono text-[12px] text-foreground-lighter">
            {'•'.repeat(10)}
            {token.slice(-4)}
          </span>
        </div>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void navigate('/server')}>
          <Server /> Server details
        </DropdownMenuItem>
        <DropdownMenuItem asChild>
          <a href={DOCS_URL} target="_blank" rel="noreferrer">
            <BookOpen /> Documentation
          </a>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={signOut}>
          <LogOut /> Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/**
 * Studio top bar: logo / server ⇕ / resource ⇕ / type + state pills · Connect
 * … Docs · Search ⌘K · theme · help · sign out · account.
 */
export function TopBar({ onOpenMobileNav }: { onOpenMobileNav: () => void }) {
  const resource = useRouteResource()
  const openCommand = useUi((s) => s.setCommandOpen)
  const signOut = useAuth((s) => s.signOut)

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b bg-background pr-3 pl-2 md:pr-4 md:pl-3">
      <Button
        variant="ghost"
        size="icon"
        className="md:hidden"
        icon={<Menu />}
        aria-label="Open navigation"
        onClick={onOpenMobileNav}
      />
      <nav aria-label="Breadcrumb" className="flex min-w-0 flex-1 items-center gap-0.5">
        <Link
          to="/services"
          className="flex size-8 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="Ferry home"
        >
          <FerryLogo className="size-5" />
        </Link>
        <span className={cn('flex min-w-0 items-center gap-0.5', resource.kind && 'max-sm:hidden')}>
          <Slash />
          <ServerSegment />
        </span>
        {resource.kind === 'service' && <ServiceSegment name={resource.name} />}
        {resource.kind === 'datastore' && <DatastoreSegment name={resource.name} />}
        {resource.kind === 'env-group' && <EnvGroupSegment name={resource.name} />}
        <span className="ml-2 hidden md:inline-flex">
          <ConnectPopover />
        </span>
      </nav>

      <div className="flex shrink-0 items-center gap-2">
        <Button asChild variant="ghost" size="sm" className="hidden text-foreground-light xl:inline-flex">
          <a href={DOCS_URL} target="_blank" rel="noreferrer">
            Docs
          </a>
        </Button>
        <button
          type="button"
          onClick={() => openCommand(true)}
          aria-label="Search (⌘K)"
          aria-keyshortcuts={isMac ? 'Meta+K' : 'Control+K'}
          className="hidden h-8 w-44 cursor-pointer items-center gap-2 rounded-full border border-border-strong bg-surface-100 pr-1.5 pl-3 text-[13px] text-foreground-lighter outline-none transition-colors hover:border-border-stronger hover:text-foreground-light focus-visible:ring-2 focus-visible:ring-ring sm:flex dark:bg-surface-200"
        >
          <Search className="size-3.5" aria-hidden="true" />
          <span className="flex-1 text-left">Search…</span>
          <Kbd className="rounded-full px-1.5">{isMac ? '⌘K' : 'Ctrl K'}</Kbd>
        </button>
        <Button
          variant="outline"
          size="icon-lg"
          className={cn(iconButton, 'sm:hidden')}
          icon={<Search />}
          aria-label="Search"
          onClick={() => openCommand(true)}
        />
        <ThemeMenu className="max-sm:hidden" />
        <Hint label="Help & documentation">
          <Button asChild variant="outline" size="icon-lg" className={cn(iconButton, 'hidden sm:inline-flex')}>
            <a href={DOCS_URL} target="_blank" rel="noreferrer" aria-label="Help (opens the README)">
              <CircleHelp />
            </a>
          </Button>
        </Hint>
        <Hint label="Sign out">
          <Button
            variant="outline"
            size="icon-lg"
            className={cn(iconButton, 'hidden sm:inline-flex')}
            icon={<LogOut />}
            aria-label="Sign out"
            onClick={signOut}
          />
        </Hint>
        <AccountMenu />
      </div>
    </header>
  )
}
