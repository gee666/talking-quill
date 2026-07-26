import { useMemo, useState, type KeyboardEvent } from 'react';
import type { ProviderCatalogEntry } from '../../../shared/schemas/providers';
import { EmptyState, Input } from '../../design';
import { PROVIDER_LOGOS } from '../provider-logos';

export function ProviderPicker({
  providers,
  selected,
  disabled,
  onSelect,
}: {
  readonly providers: readonly ProviderCatalogEntry[];
  readonly selected: ProviderCatalogEntry;
  readonly disabled: boolean;
  readonly onSelect: (provider: ProviderCatalogEntry) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [activeOption, setActiveOption] = useState(0);
  const filtered = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return query.length === 0
      ? providers
      : providers.filter((provider) =>
          `${provider.displayName} ${provider.description}`.toLocaleLowerCase().includes(query),
        );
  }, [providers, search]);

  const choose = (provider: ProviderCatalogEntry) => {
    setOpen(false);
    setSearch('');
    setActiveOption(0);
    onSelect(provider);
  };
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (filtered.length === 0) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const direction = event.key === 'ArrowDown' ? 1 : -1;
      setActiveOption((current) => (current + direction + filtered.length) % filtered.length);
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      const provider = filtered[activeOption];
      if (provider !== undefined) choose(provider);
    }
    if (event.key === 'Escape') setOpen(false);
  };

  return (
    <div className="provider-picker">
      <span className="provider-picker__label">Provider</span>
      <button
        type="button"
        className="provider-picker__trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        <img src={PROVIDER_LOGOS[selected.id]} alt="" />
        <span>
          <strong>{selected.displayName}</strong>
          <small>{selected.description}</small>
        </span>
        <span aria-hidden="true">⌄</span>
      </button>
      {open ? (
        <div className="provider-picker__popover">
          <Input
            label="Search providers"
            type="search"
            value={search}
            onChange={(event) => {
              setSearch(event.currentTarget.value);
              setActiveOption(0);
            }}
          />
          <div
            className="provider-picker__list"
            role="listbox"
            aria-label="Smart processing providers"
            aria-activedescendant={filtered[activeOption]?.id}
            tabIndex={0}
            onKeyDown={handleKeyDown}
          >
            {filtered.map((provider, index) => (
              <button
                id={provider.id}
                key={provider.id}
                type="button"
                role="option"
                aria-selected={provider.id === selected.id}
                className={index === activeOption ? 'provider-option is-active' : 'provider-option'}
                onMouseEnter={() => setActiveOption(index)}
                onClick={() => choose(provider)}
              >
                <img src={PROVIDER_LOGOS[provider.id]} alt="" />
                <span>
                  <strong>{provider.displayName}</strong>
                  <small>{provider.description}</small>
                </span>
              </button>
            ))}
            {filtered.length === 0 ? (
              <EmptyState
                title="No matching providers"
                description="Try a provider name or clear the search."
              />
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}
