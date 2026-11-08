export function customElement(tag: string) {
  return (target: CustomElementConstructor) => customElements.define(tag, target);
}
