export function titleCase(text) {
  return text.replace(/\b\w/g, (letter) => letter.toUpperCase());
}
