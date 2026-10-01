import { createBrowserRouter, Navigate, type RouteObject } from 'react-router'

import { LoginPage } from '@/pages/login/LoginPage'
import { NotFoundPage } from '@/pages/misc/NotFoundPage'
import { RouteError } from '@/pages/misc/RouteError'
import { ServiceLayout } from '@/pages/service/ServiceLayout'

import { FullPageLoader } from './FullPageLoader'
import { RequireAuth } from './RequireAuth'

/**
 * Pages are code-split: each file exports a named component (`XxxPage`),
 * loaded on first visit.
 */
export const routes: RouteObject[] = [
  { path: '/login', element: <LoginPage />, errorElement: <RouteError /> },
  {
    // The first run of a server: the link ferryd prints leads here.
    path: '/setup',
    errorElement: <RouteError />,
    hydrateFallbackElement: <FullPageLoader />,
    lazy: async () => ({ Component: (await import('@/pages/login/SetupPage')).SetupPage }),
  },
  {
    // Where `ferry login` sends the browser: outside the shell, it is one
    // decision; it checks the session itself.
    path: '/cli-login',
    errorElement: <RouteError />,
    hydrateFallbackElement: <FullPageLoader />,
    lazy: async () => ({ Component: (await import('@/pages/login/CliLoginPage')).CliLoginPage }),
  },
  {
    // Where GitHub / GitLab send the browser back: outside the shell (it is
    // often a small window of its own); it checks the session itself.
    path: '/git/callback',
    errorElement: <RouteError />,
    hydrateFallbackElement: <FullPageLoader />,
    lazy: async () => ({ Component: (await import('@/pages/git/GitCallbackPage')).GitCallbackPage }),
  },
  {
    element: <RequireAuth />,
    errorElement: <RouteError />,
    hydrateFallbackElement: <FullPageLoader />,
    children: [
      {
        errorElement: <RouteError />,
        children: [
          { index: true, element: <Navigate to="/services" replace /> },
          {
            path: 'services',
            lazy: async () => ({ Component: (await import('@/pages/services/ServicesPage')).ServicesPage }),
          },
          {
            path: 'services/new',
            lazy: async () => ({ Component: (await import('@/pages/services/NewServicePage')).NewServicePage }),
          },
          {
            path: 'services/:name',
            element: <ServiceLayout />,
            children: [
              {
                index: true,
                lazy: async () => ({ Component: (await import('@/pages/service/OverviewPage')).OverviewPage }),
              },
              {
                path: 'deploys',
                lazy: async () => ({ Component: (await import('@/pages/service/DeploysPage')).DeploysPage }),
              },
              {
                path: 'deploys/:deployId',
                lazy: async () => ({ Component: (await import('@/pages/service/DeployDetailPage')).DeployDetailPage }),
              },
              {
                path: 'logs',
                lazy: async () => ({ Component: (await import('@/pages/service/LogsPage')).LogsPage }),
              },
              {
                path: 'jobs',
                lazy: async () => ({ Component: (await import('@/pages/service/JobsPage')).JobsPage }),
              },
              {
                path: 'jobs/:jobId',
                lazy: async () => ({ Component: (await import('@/pages/service/JobDetailPage')).JobDetailPage }),
              },
              {
                path: 'environment',
                lazy: async () => ({ Component: (await import('@/pages/service/EnvironmentPage')).EnvironmentPage }),
              },
              {
                path: 'settings',
                lazy: async () => ({ Component: (await import('@/pages/service/SettingsPage')).SettingsPage }),
              },
              { path: '*', element: <NotFoundPage /> },
            ],
          },
          {
            path: 'datastores',
            lazy: async () => ({ Component: (await import('@/pages/datastores/DatastoresPage')).DatastoresPage }),
          },
          {
            path: 'datastores/:name',
            lazy: async () => ({
              Component: (await import('@/pages/datastores/DatastoreDetailPage')).DatastoreDetailPage,
            }),
          },
          {
            path: 'env-groups',
            lazy: async () => ({ Component: (await import('@/pages/env-groups/EnvGroupsPage')).EnvGroupsPage }),
          },
          {
            path: 'env-groups/:name',
            lazy: async () => ({
              Component: (await import('@/pages/env-groups/EnvGroupDetailPage')).EnvGroupDetailPage,
            }),
          },
          {
            path: 'blueprints',
            lazy: async () => ({ Component: (await import('@/pages/blueprints/BlueprintsPage')).BlueprintsPage }),
          },
          {
            path: 'server',
            lazy: async () => ({ Component: (await import('@/pages/server/ServerPage')).ServerPage }),
          },
          ...(import.meta.env.DEV
            ? [
                {
                  path: 'dev/patterns',
                  lazy: async () => ({ Component: (await import('@/pages/dev/PatternsPage')).PatternsPage }),
                },
              ]
            : []),
          { path: '*', element: <NotFoundPage /> },
        ],
      },
    ],
  },
]

export function createRouter() {
  return createBrowserRouter(routes)
}
