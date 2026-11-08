export type DefinedState = 'on' | 'off';

export class Defined extends HTMLElement {
  state: DefinedState = 'on';
}

customElements.define('x-defined', Defined);
