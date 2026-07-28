import type { RefObject } from 'react';
import { DictationHistory } from '../history/DictationHistory';

export function HistoryScreen({
  headingRef,
}: {
  readonly headingRef: RefObject<HTMLHeadingElement | null>;
}) {
  return (
    <div className="screen">
      <header className="screen__header">
        <div>
          <p className="eyebrow">History</p>
          <h1 ref={headingRef} tabIndex={-1}>
            Dictation history
          </h1>
          <p>Everything you have dictated, kept only on your computer.</p>
        </div>
      </header>
      <div className="screen__grid">
        <DictationHistory showHeading={false} showDescription={false} />
      </div>
    </div>
  );
}
