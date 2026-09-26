import { Hammer } from 'lucide-react'

import { EmptyState } from '@/components/patterns/EmptyState'
import { PageSection } from '@/components/patterns/Page'

/** Temporary body for service sub-pages another agent is building. */
export function SectionPlaceholder({ title }: { title: string }) {
  return (
    <PageSection title={title}>
      <EmptyState icon={<Hammer />} title="Coming soon" description="This page is being built." />
    </PageSection>
  )
}
