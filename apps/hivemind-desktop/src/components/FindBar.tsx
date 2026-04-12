import { type JSX } from 'solid-js';
import { ChevronUp, ChevronDown, X } from 'lucide-solid';

export interface FindBarProps {
  query: string;
  onQueryChange: (query: string) => void;
  matchCount: number;
  currentMatch: number;
  onNext: () => void;
  onPrev: () => void;
  onClose: () => void;
  /** Ref callback for the text input element. */
  inputRef?: (el: HTMLInputElement) => void;
}

const FindBar = (props: FindBarProps): JSX.Element => {
  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      props.onClose();
    } else if (e.key === 'Enter' && e.shiftKey) {
      e.preventDefault();
      props.onPrev();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      props.onNext();
    }
  };

  return (
    <div class="find-bar flex items-center gap-1 px-2 py-1 border-b border-border bg-muted/50">
      <input
        ref={props.inputRef}
        type="text"
        class="find-bar-input flex-1 h-7 px-2 text-sm rounded border border-border bg-background text-foreground outline-none focus:ring-1 focus:ring-ring"
        placeholder="Find…"
        value={props.query}
        onInput={(e) => props.onQueryChange(e.currentTarget.value)}
        onKeyDown={handleKeyDown}
      />
      <span class="find-bar-count text-xs text-muted-foreground whitespace-nowrap min-w-[4rem] text-center">
        {props.matchCount > 0
          ? `${props.currentMatch} of ${props.matchCount}`
          : props.query
            ? 'No results'
            : ''}
      </span>
      <button
        class="find-bar-btn p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground disabled:opacity-40"
        onClick={props.onPrev}
        disabled={props.matchCount === 0}
        title="Previous match (Shift+Enter)"
      >
        <ChevronUp size={14} />
      </button>
      <button
        class="find-bar-btn p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground disabled:opacity-40"
        onClick={props.onNext}
        disabled={props.matchCount === 0}
        title="Next match (Enter)"
      >
        <ChevronDown size={14} />
      </button>
      <button
        class="find-bar-btn p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground"
        onClick={props.onClose}
        title="Close (Escape)"
      >
        <X size={14} />
      </button>
    </div>
  );
};

export default FindBar;
