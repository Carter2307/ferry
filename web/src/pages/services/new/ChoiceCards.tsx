import * as React from 'react'
import { Check } from 'lucide-react'
import { RadioGroup as RadioGroupPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'

export interface Choice<T extends string> {
  value: T
  label: string
  description: React.ReactNode
  icon: React.ReactNode
}

interface ChoiceCardsProps<T extends string> {
  value: T
  onChange: (value: T) => void
  choices: readonly Choice<T>[]
  /** Accessible name of the radio group. */
  label: string
  id?: string
  className?: string
  /** `lg` = icon box + two-line description (type picker); `sm` = compact (source picker). */
  size?: 'sm' | 'lg'
}

/**
 * Radio cards (Studio "select a plan / compute size" look): keyboard
 * navigable radio group, selected card outlined in green with a check.
 */
export function ChoiceCards<T extends string>({
  value,
  onChange,
  choices,
  label,
  id,
  className,
  size = 'lg',
}: ChoiceCardsProps<T>) {
  const uid = React.useId()
  return (
    <RadioGroupPrimitive.Root
      id={id}
      value={value}
      onValueChange={(v) => {
        const choice = choices.find((c) => c.value === v)
        if (choice) onChange(choice.value)
      }}
      aria-label={label}
      className={cn('grid gap-3', className)}
    >
      {choices.map((c) => (
        <RadioGroupPrimitive.Item
          key={c.value}
          value={c.value}
          aria-label={c.label}
          aria-describedby={`${uid}-${c.value}`}
          className={cn(
            'group relative flex cursor-pointer items-start gap-3 rounded-lg border bg-surface-100 text-left shadow-card transition-[border-color,background-color,box-shadow] outline-none',
            'hover:border-border-stronger focus-visible:ring-2 focus-visible:ring-ring',
            'data-[state=checked]:border-primary data-[state=checked]:ring-1 data-[state=checked]:ring-primary/40',
            size === 'lg' ? 'p-4' : 'px-3.5 py-3',
          )}
        >
          <span
            aria-hidden="true"
            className={cn(
              'flex shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-lighter transition-colors dark:bg-surface-200',
              'group-data-[state=checked]:border-primary/40 group-data-[state=checked]:text-primary',
              size === 'lg' ? 'size-9 [&_svg]:size-[18px]' : 'size-7 [&_svg]:size-4',
            )}
          >
            {c.icon}
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5 pr-5">
            <span className="text-sm font-medium text-foreground">{c.label}</span>
            <span
              id={`${uid}-${c.value}`}
              className={cn('text-foreground-light', size === 'lg' ? 'text-[13px] leading-snug' : 'text-[12.5px] leading-snug')}
            >
              {c.description}
            </span>
          </span>
          <span
            aria-hidden="true"
            className="absolute top-3 right-3 flex size-4 items-center justify-center rounded-full border border-border-stronger transition-colors group-data-[state=checked]:border-primary-solid-border group-data-[state=checked]:bg-primary-solid"
          >
            <Check className="size-2.5 text-primary-foreground opacity-0 group-data-[state=checked]:opacity-100" strokeWidth={3.5} />
          </span>
        </RadioGroupPrimitive.Item>
      ))}
    </RadioGroupPrimitive.Root>
  )
}
