import { LitElement } from 'lit';

export type UnregisteredState = 'on' | 'off';

export class Unregistered extends LitElement {
  state: UnregisteredState = 'on';
}
