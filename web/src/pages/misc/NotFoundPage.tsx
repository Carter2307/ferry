import { Link } from 'react-router'
import { Compass } from 'lucide-react'

import { EmptyState } from '@/components/patterns/EmptyState'
import { PageContainer } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'

/** Unknown route (inside the shell). */
export function NotFoundPage() {
  return (
    <PageContainer size="narrow" className="flex flex-1 items-center">
      <EmptyState
        size="lg"
        icon={<Compass />}
        title="Page not found"
        description="The page you are looking for doesn't exist or was moved."
        actions={
          <Button asChild variant="primary">
            <Link to="/services">Go to services</Link>
          </Button>
        }
      />
    </PageContainer>
  )
}
