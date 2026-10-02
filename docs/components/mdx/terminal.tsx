'use client';
import { Check, Copy, RotateCcw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/cn';
import { useOnScreen, useReducedMotion } from './figure';

type Line = { kind: 'cmd' | 'out' | 'note'; text: string };

/** `$ …` is a command (typed), `# …` a remark, anything else is output. */
function parse(lines: string[]): Line[] {
  return lines.map((line) => {
    if (line.startsWith('$ ')) return { kind: 'cmd', text: line.slice(2) };
    if (line.startsWith('# ')) return { kind: 'note', text: line };
    return { kind: 'out', text: line };
  });
}

const TYPE_MS = 28;
const OUT_MS = 220;
const AFTER_CMD_MS = 420;

const buttonClass =
  'inline-flex size-7 items-center justify-center rounded-[var(--ferry-radius-md)] text-foreground-lighter transition-colors hover:bg-selection hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[var(--ring)] [&_svg]:size-3.5';

/**
 * A terminal session that plays when it comes on screen: commands are typed,
 * then their output shows line by line. The copy button copies the commands.
 *
 * <Terminal lines={['$ ferry up hello', '==> Build successful', '==> Your service is live']} />
 */
export function Terminal({ lines, title = 'Terminal' }: { lines: string[]; title?: string }) {
  const script = parse(lines);
  const ref = useRef<HTMLDivElement>(null);
  const visible = useOnScreen(ref, 0.5);
  const reduced = useReducedMotion();
  // `line` lines are complete; `chars` characters of the next command are typed.
  const [line, setLine] = useState(0);
  const [chars, setChars] = useState(0);
  const [started, setStarted] = useState(false);
  const [copied, setCopied] = useState(false);
  const done = reduced || line >= script.length;

  useEffect(() => {
    if (visible) setStarted(true);
  }, [visible]);

  useEffect(() => {
    if (!started || done) return;
    const current = script[line];
    if (!current) return;
    let wait = OUT_MS;
    let next = () => setLine((l) => l + 1);
    if (current.kind === 'cmd') {
      if (chars < current.text.length) {
        wait = TYPE_MS;
        next = () => setChars((c) => c + 1);
      } else {
        wait = AFTER_CMD_MS;
        next = () => {
          setChars(0);
          setLine((l) => l + 1);
        };
      }
    }
    const timer = window.setTimeout(next, wait);
    return () => window.clearTimeout(timer);
    // `script` comes from the props: the two indexes are the state.
  }, [started, done, line, chars]);

  const commands = script
    .filter((l) => l.kind === 'cmd')
    .map((l) => l.text)
    .join('\n');

  return (
    <div
      ref={ref}
      className="not-prose my-6 overflow-hidden rounded-[var(--ferry-radius-lg)] border border-border bg-surface-75"
    >
      <div className="flex h-9 items-center justify-between border-b border-border ps-4 pe-1.5">
        <span className="text-[13px] text-foreground-lighter">{title}</span>
        <span className="flex items-center gap-0.5">
          <button
            type="button"
            className={buttonClass}
            aria-label="Play again"
            onClick={() => {
              setLine(0);
              setChars(0);
              setStarted(true);
            }}
          >
            <RotateCcw aria-hidden="true" />
          </button>
          <button
            type="button"
            className={buttonClass}
            aria-label="Copy the commands"
            onClick={() => {
              void navigator.clipboard?.writeText(commands).then(() => {
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              });
            }}
          >
            {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
          </button>
        </span>
      </div>
      <pre className="m-0 overflow-x-auto px-4 py-3.5 font-mono text-[13px] leading-[1.65]">
        <code>
          {script.map((l, i) => {
            const complete = done || i < line;
            const typing = !done && i === line && l.kind === 'cmd';
            const shown = complete || typing;
            const text = typing ? l.text.slice(0, chars) : l.text;
            return (
              // Every line keeps its place from the start, so the page never moves.
              <span key={i} className={cn('block min-h-[1lh] whitespace-pre', !shown && 'invisible')}>
                {l.kind === 'cmd' ? (
                  <>
                    <span className="text-foreground-muted select-none">$ </span>
                    <span className="text-foreground">{text}</span>
                    {typing ? <span className="ferry-caret" aria-hidden="true" /> : null}
                    {typing ? <span className="invisible">{l.text.slice(chars)}</span> : null}
                  </>
                ) : (
                  <span
                    className={cn(
                      l.kind === 'note' && 'text-foreground-muted',
                      l.kind === 'out' && (l.text.startsWith('error') || l.text.startsWith('✗')
                        ? 'text-destructive'
                        : 'text-foreground-light'),
                    )}
                  >
                    {text || ' '}
                  </span>
                )}
              </span>
            );
          })}
        </code>
      </pre>
    </div>
  );
}
