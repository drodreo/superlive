/**
 * Text - plain text display (the A2UI vocabulary's label/heading carrier).
 *
 * Official A2UI basic catalog parity: `text` is the required content, `variant`
 * hints the base text style (h1-h5/body/caption, default body). Pure display —
 * no margin/padding (components never carry outer spacing; containers own it),
 * no interaction, no protocol awareness.
 */

export type TextVariant = "h1" | "h2" | "h3" | "h4" | "h5" | "caption" | "body";

export interface TextProps {
  text: string;
  variant?: TextVariant;
}

const HEADING_TAGS = new Set(["h1", "h2", "h3", "h4", "h5"]);

export function Text({ text, variant = "body" }: TextProps) {
  const Tag = HEADING_TAGS.has(variant)
    ? (variant as "h1" | "h2" | "h3" | "h4" | "h5")
    : "p";
  return <Tag className={`sl-ui-text sl-ui-text--${variant}`}>{text}</Tag>;
}
