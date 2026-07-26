import { describe, expect, it } from 'vitest';
import { findSecretRuleIds } from '../../scripts/secret-rules.mjs';

describe('high-confidence shared secret signatures', () => {
  it('detects modern source-control and payment credentials', () => {
    const source = [
      ['github', '_pat_', 'A'.repeat(30)].join(''),
      ['glpat', '-', 'B'.repeat(24)].join(''),
      ['sk', '_live_', 'C'.repeat(24)].join(''),
    ].join('\n');
    expect(findSecretRuleIds(source)).toEqual(['github-token', 'gitlab-token', 'stripe-live-key']);
  });

  it('does not classify short documentation placeholders as credentials', () => {
    expect(findSecretRuleIds('github_pat_example glpat-example sk_live_example')).toEqual([]);
  });
});
