import { Component, createMemo, type JSX } from 'solid-js';

/**
 * A textarea with a backdrop overlay that highlights `{{handlebars}}`
 * template tags.  The textarea renders the visible text normally; the
 * backdrop behind it provides coloured background rectangles behind
 * template tags (its own text is invisible).  Because the textarea's
 * background is transparent, the tag highlights show through.
 */

export interface TemplateTextareaProps {
  ref?: (el: HTMLTextAreaElement) => void;
  style?: JSX.CSSProperties;
  value?: string;
  onInput?: (e: InputEvent & { currentTarget: HTMLTextAreaElement }) => void;
  onBlur?: () => void;
  disabled?: boolean;
  placeholder?: string;
}

// Split text into segments of plain text and `{{…}}` tokens.
function tokenize(text: string): { text: string; isTag: boolean }[] {
  const tokens: { text: string; isTag: boolean }[] = [];
  const re = /\{\{[^}]*\}\}/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) tokens.push({ text: text.slice(last, m.index), isTag: false });
    tokens.push({ text: m[0], isTag: true });
    last = m.index + m[0].length;
  }
  if (last < text.length) tokens.push({ text: text.slice(last), isTag: false });
  // Trailing newline: browsers collapse a final \n in a div so the
  // backdrop would be one line shorter than the textarea.  Append a
  // zero-width space to force the extra line.
  if (text.endsWith('\n') || text === '') tokens.push({ text: '\u200b', isTag: false });
  return tokens;
}

// Visible background behind tags — no layout-affecting properties.
const TAG_STYLE: JSX.CSSProperties = {
  background: 'hsl(207 80% 55% / 0.25)',
  'border-radius': '3px',
};

const TemplateTextarea: Component<TemplateTextareaProps> = (props) => {
  let textareaRef: HTMLTextAreaElement | undefined;
  let backdropRef: HTMLDivElement | undefined;

  const tokens = createMemo(() => tokenize(props.value ?? ''));

  const syncScroll = () => {
    if (backdropRef && textareaRef) {
      backdropRef.scrollTop = textareaRef.scrollTop;
      backdropRef.scrollLeft = textareaRef.scrollLeft;
    }
  };

  // Shared typographic styles that MUST match between backdrop and
  // textarea so highlighted tokens stay pixel-aligned.
  const sharedTypography = (): JSX.CSSProperties => ({
    'font-family': 'inherit',
    'font-size': props.style?.['font-size'] ?? '0.85em',
    'line-height': '1.45',
    'letter-spacing': 'normal',
    'white-space': 'pre-wrap',
    'word-wrap': 'break-word',
    'overflow-wrap': 'break-word',
    padding: props.style?.padding ?? '4px 8px',
  });

  return (
    <div style={{
      position: 'relative',
      width: props.style?.width ?? '100%',
    }}>
      {/* Backdrop: invisible text with visible background highlights on
          {{tags}}.  Sits behind the transparent-background textarea. */}
      <div
        ref={(el) => { backdropRef = el; }}
        aria-hidden="true"
        style={{
          ...sharedTypography(),
          position: 'absolute',
          inset: '0',
          overflow: 'hidden',
          'pointer-events': 'none',
          color: 'transparent',
          border: props.style?.border ?? '1px solid transparent',
          'border-radius': props.style?.['border-radius'] ?? '4px',
          'box-sizing': 'border-box',
        }}
      >
        {tokens().map((t) =>
          t.isTag
            ? <span style={TAG_STYLE}>{t.text}</span>
            : t.text
        )}
      </div>

      {/* Actual textarea — normal visible text, transparent background
          so tag highlights from the backdrop show through. */}
      <textarea
        ref={(el) => {
          textareaRef = el;
          props.ref?.(el);
        }}
        style={{
          ...props.style,
          ...sharedTypography(),
          background: 'transparent',
          position: 'relative',
          'z-index': 1,
        }}
        value={props.value ?? ''}
        onInput={(e) => {
          props.onInput?.(e);
          syncScroll();
        }}
        onBlur={props.onBlur}
        onScroll={syncScroll}
        disabled={props.disabled}
        placeholder={props.placeholder}
        spellcheck={false}
        autocapitalize="off"
      />
    </div>
  );
};

export default TemplateTextarea;
