/**
 * LoadingSpinner - loading component (fixed to screen center + translucent overlay). Verbatim from superlive.
 * The size modifier uses dedicated classes sl-ui-loading-spinner--sm / --lg (class+class compounds banned).
 */
export interface LoadingSpinnerProps {
  size?: 'sm' | 'md' | 'lg';
  message?: string;
  overlay?: boolean;
}

const sizeClass = {
  sm: 'sl-ui-loading-spinner--sm',
  md: '',
  lg: 'sl-ui-loading-spinner--lg',
};

export function LoadingSpinner({
  size = 'md',
  message,
  overlay = true,
}: LoadingSpinnerProps) {
  const spinner = (
    <>
      <div className={`sl-ui-loading-spinner${sizeClass[size] ? ' ' + sizeClass[size] : ''}`} />
      {message && <span className="sl-ui-loading-message">{message}</span>}
    </>
  );

  if (!overlay) return spinner;

  return (
    <div className="sl-ui-loading-overlay" role="status" aria-live="polite">
      {spinner}
    </div>
  );
}

export default LoadingSpinner;
