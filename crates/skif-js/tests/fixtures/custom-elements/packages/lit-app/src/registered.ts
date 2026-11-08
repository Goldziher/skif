import { LitElement, html } from 'lit';
import { customElement } from 'lit/decorators.js';

@customElement('x-registered')
export class Registered extends LitElement {
  render() {
    return html`<p>registered</p>`;
  }
}
