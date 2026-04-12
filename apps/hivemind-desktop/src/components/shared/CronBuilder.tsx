import { For, createMemo } from 'solid-js';

export interface CronBuilderProps {
  value: string;
  onChange: (cron: string) => void;
  disabled?: boolean;
}

const PRESETS = [
  { label: 'Every minute', cron: '0 * * * * * *' },
  { label: 'Every 5 min', cron: '0 */5 * * * * *' },
  { label: 'Every 15 min', cron: '0 */15 * * * * *' },
  { label: 'Every hour', cron: '0 0 * * * * *' },
  { label: 'Every 6h', cron: '0 0 */6 * * * *' },
  { label: 'Daily midnight', cron: '0 0 0 * * * *' },
  { label: 'Daily 9 AM', cron: '0 0 9 * * * *' },
  { label: 'Weekly Mon', cron: '0 0 0 * * MON *' },
  { label: 'Monthly 1st', cron: '0 0 0 1 * * *' },
];

const DOW_NAMES = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT'] as const;
const DOW_SHORT = ['S', 'M', 'T', 'W', 'T', 'F', 'S'] as const;

const FIELDS = [
  { label: 'Sec', idx: 0 }, { label: 'Min', idx: 1 }, { label: 'Hour', idx: 2 },
  { label: 'Day', idx: 3 }, { label: 'Month', idx: 4 }, { label: 'Year', idx: 6 },
];

function parseParts(value: string): string[] {
  const p = value.split(/\s+/);
  while (p.length < 7) p.push('*');
  return p;
}

function parseDowSelection(parts: string[]): Set<string> {
  const raw = parts[5];
  if (!raw || raw === '*' || raw === '?') return new Set();
  const selected = new Set<string>();
  for (const p of raw.split(',').map(s => s.trim().toUpperCase())) {
    if ((DOW_NAMES as readonly string[]).includes(p)) selected.add(p);
  }
  return selected;
}

export function describeCron(value: string): string {
  const p = parseParts(value);
  const [sec, min, hr, dom, mon, dow] = p;
  const parts: string[] = [];
  if (sec !== '0' && sec !== '*') parts.push(`sec ${sec}`);
  if (min === '*') parts.push('every minute');
  else if (min.startsWith('*/')) parts.push(`every ${min.slice(2)} min`);
  else parts.push(`at min ${min}`);
  if (hr !== '*') {
    if (hr.startsWith('*/')) parts.push(`every ${hr.slice(2)}h`);
    else parts.push(`at ${hr.padStart(2, '0')}:${min === '*' ? '00' : min.padStart(2, '0')}`);
  }
  if (dom !== '*') parts.push(`day ${dom}`);
  if (mon !== '*') parts.push(`month ${mon}`);
  if (dow !== '*') parts.push(`${dow}`);
  return parts.join(', ');
}

const inputBase = 'width:100%;text-align:center;padding:4px 2px;font-size:0.78em;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));box-sizing:border-box;';

export default function CronBuilder(props: CronBuilderProps) {
  const parts = createMemo(() => parseParts(props.value));
  const ro = () => props.disabled ?? false;

  function setField(idx: number, value: string) {
    const p = [...parts()];
    p[idx] = value || '*';
    props.onChange(p.join(' '));
  }

  function toggleDow(day: string) {
    if (ro()) return;
    const sel = parseDowSelection(parts());
    if (sel.has(day)) sel.delete(day); else sel.add(day);
    if (sel.size === 0 || sel.size === 7) setField(5, '*');
    else setField(5, DOW_NAMES.filter(d => sel.has(d)).join(','));
  }

  return (
    <div style="padding:10px;border:1px solid hsl(var(--border));border-radius:6px;background:hsl(var(--background));display:flex;flex-direction:column;gap:8px;">
      {/* Presets */}
      <div style="display:flex;flex-wrap:wrap;gap:4px;">
        <For each={PRESETS}>
          {(p) => (
            <button
              style={{
                background: props.value === p.cron ? 'hsl(var(--primary))' : 'transparent',
                color: props.value === p.cron ? 'hsl(var(--primary-foreground))' : 'hsl(var(--foreground))',
                border: '1px solid hsl(var(--border))', 'border-radius': '4px',
                padding: '2px 8px', 'font-size': '0.72em', cursor: ro() ? 'default' : 'pointer',
              }}
              onClick={() => { if (!ro()) props.onChange(p.cron); }}
              disabled={ro()}
            >{p.label}</button>
          )}
        </For>
      </div>

      {/* Field editors */}
      <div style="display:flex;gap:4px;">
        <For each={FIELDS}>
          {(f) => (
            <div style="display:flex;flex-direction:column;align-items:center;flex:1;">
              <input
                style={inputBase}
                value={parts()[f.idx]}
                onInput={(e) => setField(f.idx, e.currentTarget.value)}
                disabled={ro()}
              />
              <span style="font-size:0.6em;color:hsl(var(--muted-foreground));margin-top:1px;">{f.label}</span>
            </div>
          )}
        </For>
      </div>

      {/* Day of Week toggles */}
      <div>
        <span style="font-size:0.72em;color:hsl(var(--muted-foreground));">Day of Week</span>
        <div style="display:flex;gap:0;margin-top:3px;">
          <For each={DOW_NAMES.slice()}>
            {(day, idx) => {
              const isSelected = () => parseDowSelection(parts()).has(day);
              const isAny = () => { const raw = parts()[5]; return !raw || raw === '*' || raw === '?'; };
              return (
                <button
                  style={{
                    flex: '1', padding: '5px 0', 'font-size': '0.7em',
                    'font-weight': isSelected() ? '700' : '400',
                    background: isSelected() ? 'hsl(var(--primary))' : 'transparent',
                    color: isSelected() ? 'hsl(var(--primary-foreground))' : isAny() ? 'hsl(var(--primary))' : 'hsl(var(--muted-foreground))',
                    border: isSelected() ? '1px solid hsl(var(--primary))' : '1px solid hsl(var(--border))',
                    'border-radius': idx() === 0 ? '4px 0 0 4px' : idx() === 6 ? '0 4px 4px 0' : '0',
                    cursor: ro() ? 'default' : 'pointer', transition: 'all 0.15s ease',
                  }}
                  title={day}
                  onClick={() => toggleDow(day)}
                  disabled={ro()}
                >{DOW_SHORT[idx()]}</button>
              );
            }}
          </For>
        </div>
        <div style="font-size:0.6em;color:hsl(var(--muted-foreground));margin-top:2px;">
          {(() => { const sel = parseDowSelection(parts()); const raw = parts()[5]; if (!raw || raw === '*' || raw === '?') return 'Any day (click to restrict)'; if (sel.size === 0) return raw; return [...sel].join(', '); })()}
        </div>
      </div>

      {/* Description */}
      <div style="font-size:0.7em;color:hsl(var(--muted-foreground));">
        {props.value ? describeCron(props.value) : 'No schedule set'}
      </div>
      <div style="font-size:0.6em;color:hsl(var(--muted-foreground));font-family:monospace;">
        Format: sec min hour day month dow year — use * for any, */N for every N
      </div>
    </div>
  );
}
