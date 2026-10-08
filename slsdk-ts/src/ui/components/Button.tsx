/**
 * Button (@shell/sdk/ui shared facility): 5 variants + 3 sizes + icon-only + loading.
 * lucide-react is not on the component dependency whitelist — icon accepts any ReactNode (callers pass inline SVG);
 * the loading spinner is a CSS border circle (span.sl-ui-btn__spinner + keyframes).
 * Class name contract: .sl-ui-* (the architecture.md CSS contract, low-specificity single-class selectors).
 */
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'info';
export type ButtonSize = 'sm' | 'md' | 'lg';

export interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'type'> {
  /** Visual variant. Defaults to `secondary` */
  variant?: ButtonVariant;
  /** Size tier. Defaults to `md` (32px) */
  size?: ButtonSize;
  /** Icon node (inline SVG etc.). Passing `icon` without `children` → automatically a 1:1 square */
  icon?: ReactNode;
  /** Icon position, defaults to `left` */
  iconPosition?: 'left' | 'right';
  /** Loading state: shows the spinner + disables the button */
  loading?: boolean;
  /** width: 100% */
  block?: boolean;
  /** Button type. Defaults to `button` (avoids being misread as submit inside forms) */
  type?: 'button' | 'submit' | 'reset';
  children?: ReactNode;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = 'secondary',
    size = 'md',
    icon,
    iconPosition = 'left',
    loading = false,
    block = false,
    type = 'button',
    disabled,
    className,
    children,
    ...rest
  },
  ref,
) {
  // icon-only auto-detection: icon passed without children → 1:1 square
  const iconOnly = !!icon && children == null;

  const classes = [
    'sl-ui-btn',
    `sl-ui-btn--${variant}`,
    `sl-ui-btn--${size}`,
    iconOnly && 'sl-ui-btn--icon',
    block && 'sl-ui-btn--block',
    loading && 'sl-ui-btn--loading',
    className,
  ]
    .filter(Boolean)
    .join(' ');

  const renderIcon = (position: 'left' | 'right') => {
    if (loading && position === 'left') {
      return <span className="sl-ui-btn__spinner" aria-hidden="true" />;
    }
    if (loading || !icon || iconPosition !== position) return null;
    return <span className="sl-ui-btn__icon">{icon}</span>;
  };

  return (
    <button
      ref={ref}
      type={type}
      className={classes}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      {renderIcon('left')}
      {children != null && <span className="sl-ui-btn__label">{children}</span>}
      {renderIcon('right')}
    </button>
  );
});
