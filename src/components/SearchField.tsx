export function SearchField({
  id,
  label,
  value,
  onChange,
  placeholder,
  className = "collection-search",
  controlsClassName,
  clearLabel = "Clear search",
  clearAriaLabel,
  alwaysShowClear = false,
  status,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  className?: string;
  controlsClassName?: string;
  clearLabel?: string;
  clearAriaLabel?: string;
  alwaysShowClear?: boolean;
  status?: string;
}) {
  return (
    <div className={className}>
      <label htmlFor={id}>{label}</label>
      <div className={controlsClassName}>
        <input
          id={id}
          type="search"
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder={placeholder}
        />
        {(alwaysShowClear || value) && (
          <button
            type="button"
            className="text-button"
            onClick={() => onChange("")}
            disabled={alwaysShowClear && !value}
            aria-label={clearAriaLabel}
          >
            {clearLabel}
          </button>
        )}
      </div>
      {status && <p role="status">{status}</p>}
    </div>
  );
}
