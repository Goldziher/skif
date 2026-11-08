export type CommentQuoteProps = { text: string };

export const Comment = (body: string) => `<article>${body}</article>`;

export const CommentQuote = ({ text }: CommentQuoteProps) => `<blockquote>${text}</blockquote>`;
