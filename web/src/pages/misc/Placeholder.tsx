import { Hammer } from 'lucide-react'

import { EmptyState } from '@/components/patterns/EmptyState'
import { PageContainer, PageHeader } from '@/components/patterns/Page'

/** Temporary body for pages another agent is building. */
export function Placeholder({ title, description }: { title: string; description?: string }) {
  return (
    <PageContainer>
      <PageHeader title={title} description={description} />
      <EmptyState icon={<Hammer />} title="Coming soon" description="This page is being built." />
    </PageContainer>
  )
}
