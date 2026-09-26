import * as React from 'react'

import { cn } from '@/lib/utils'

const LINE_HEIGHT = 20
const INDENT = '  '

export interface YamlEditorHandle {
  /** Put the caret at the start of `line` (1-based) and scroll it into view. */
  goToLine: (line: number) => void
  focus: () => void
}

interface YamlEditorProps {
  id?: string
  value: string
  onChange: (value: string) => void
  /** 1-based line flagged by the last server error. */
  errorLine?: number | null
  /** ⌘/Ctrl + Enter. */
  onSubmitShortcut?: () => void
  placeholder?: string
  className?: string
  'aria-label'?: string
  'aria-describedby'?: string
}

/**
 * Lightweight code editor: a monospace textarea with a line-number gutter
 * (kept in sync with the scroll position), current-line and error-line
 * highlighting, and 2-space indentation with Tab / Shift+Tab. Press Esc then
 * Tab to move focus out of the editor.
 */
export function YamlEditor({
  ref,
  id,
  value,
  onChange,
  errorLine,
  onSubmitShortcut,
  placeholder,
  className,
  ...aria
}: YamlEditorProps & { ref?: React.Ref<YamlEditorHandle> }) {
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null)
  const gutterRef = React.useRef<HTMLDivElement | null>(null)
  const tabEscapesRef = React.useRef(false)
  const [caretLine, setCaretLine] = React.useState(1)
  const lineCount = Math.max(1, value.split('\n').length)
  const digits = String(lineCount).length

  const syncGutter = React.useCallback(() => {
    const ta = textareaRef.current
    const gutter = gutterRef.current
    if (ta && gutter) gutter.style.transform = `translateY(${-ta.scrollTop}px)`
  }, [])

  const updateCaret = () => {
    const ta = textareaRef.current
    if (!ta) return
    setCaretLine(ta.value.slice(0, ta.selectionStart).split('\n').length)
  }

  React.useImperativeHandle(ref, () => ({
    goToLine: (line: number) => {
      const ta = textareaRef.current
      if (!ta) return
      const lines = ta.value.split('\n')
      const target = Math.min(Math.max(1, line), lines.length)
      const offset = lines.slice(0, target - 1).reduce((n, l) => n + l.length + 1, 0)
      ta.focus()
      ta.setSelectionRange(offset, offset)
      ta.scrollTop = Math.max(0, (target - 1) * LINE_HEIGHT - ta.clientHeight / 2)
      setCaretLine(target)
      syncGutter()
    },
    focus: () => textareaRef.current?.focus(),
  }))

  // Keep the gutter aligned when the content shrinks / grows.
  React.useLayoutEffect(() => syncGutter(), [value, syncGutter])

  /** Replace [start, end) with `text` keeping the browser's undo stack when possible. */
  const replaceRange = (
    ta: HTMLTextAreaElement,
    start: number,
    end: number,
    text: string,
    select?: [number, number],
  ) => {
    ta.focus()
    ta.setSelectionRange(start, end)
    const ok = typeof document.execCommand === 'function' && document.execCommand('insertText', false, text)
    if (!ok) {
      ta.setRangeText(text, start, end, 'end')
      onChange(ta.value)
    }
    if (select) ta.setSelectionRange(select[0], select[1])
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const ta = e.currentTarget
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault()
      onSubmitShortcut?.()
      return
    }
    if (e.key === 'Escape') {
      tabEscapesRef.current = true
      return
    }
    if (e.key !== 'Tab' || e.altKey || e.metaKey || e.ctrlKey) {
      tabEscapesRef.current = false
      return
    }
    if (tabEscapesRef.current) {
      // let the browser move focus
      tabEscapesRef.current = false
      return
    }
    e.preventDefault()
    const { selectionStart: start, selectionEnd: end, value: text } = ta
    const multiLine = text.slice(start, end).includes('\n')
    if (!e.shiftKey && !multiLine) {
      replaceRange(ta, start, end, INDENT)
      return
    }
    // (Out)dent every line touched by the selection.
    const blockStart = text.lastIndexOf('\n', start - 1) + 1
    const nextBreak = text.indexOf('\n', end - (end > start && text[end - 1] === '\n' ? 1 : 0))
    const blockEnd = nextBreak === -1 ? text.length : nextBreak
    const lines = text.slice(blockStart, blockEnd).split('\n')
    const changed = e.shiftKey
      ? lines.map((l) => (l.startsWith(INDENT) ? l.slice(INDENT.length) : l.replace(/^ /, '')))
      : lines.map((l) => INDENT + l)
    const block = changed.join('\n')
    if (block === lines.join('\n')) return
    const firstDelta = (changed[0]?.length ?? 0) - (lines[0]?.length ?? 0)
    const newStart = Math.max(blockStart, start + firstDelta)
    const newEnd = end + (block.length - (blockEnd - blockStart))
    replaceRange(ta, blockStart, blockEnd, block, multiLine ? [newStart, newEnd] : [newStart, newStart])
  }

  return (
    <div
      className={cn(
        'relative flex min-h-0 overflow-hidden rounded-md border border-border-strong bg-surface-100 transition-[border-color,box-shadow] dark:bg-surface-200',
        'focus-within:border-primary-bright/70 focus-within:ring-2 focus-within:ring-ring',
        className,
      )}
    >
      <div
        aria-hidden="true"
        className="relative shrink-0 overflow-hidden border-r bg-surface-200 select-none dark:bg-transparent"
        style={{ width: `${digits + 3}ch` }}
      >
        <div ref={gutterRef} className="py-3 pr-2.5 pl-2 text-right font-mono text-[12px] will-change-transform">
          {Array.from({ length: lineCount }, (_, i) => {
            const n = i + 1
            const isError = errorLine === n
            return (
              <div
                key={n}
                style={{ height: LINE_HEIGHT, lineHeight: `${LINE_HEIGHT}px` }}
                className={cn(
                  'tabular',
                  isError
                    ? '-mr-2.5 rounded-l-sm bg-destructive/10 pr-2.5 font-medium text-destructive'
                    : n === caretLine
                      ? 'text-foreground-light'
                      : 'text-foreground-muted',
                )}
              >
                {n}
              </div>
            )
          })}
        </div>
      </div>
      <textarea
        ref={textareaRef}
        id={id}
        value={value}
        placeholder={placeholder}
        onChange={(e) => {
          onChange(e.target.value)
          updateCaret()
        }}
        onScroll={syncGutter}
        onKeyDown={onKeyDown}
        onKeyUp={updateCaret}
        onClick={updateCaret}
        onSelect={updateCaret}
        wrap="off"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        autoComplete="off"
        data-gramm="false"
        aria-multiline="true"
        style={{ lineHeight: `${LINE_HEIGHT}px`, tabSize: 2 }}
        className="min-w-0 flex-1 resize-none overflow-auto bg-transparent px-3 py-3 font-mono text-[13px] whitespace-pre text-foreground outline-none placeholder:text-foreground-muted"
        {...aria}
      />
    </div>
  )
}
