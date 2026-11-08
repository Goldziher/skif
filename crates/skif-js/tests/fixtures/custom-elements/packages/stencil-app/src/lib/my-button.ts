import { Component, h } from '@stencil/core';

@Component({ tag: 'my-button' })
export class MyButton {
  render() {
    return h('button', null);
  }
}

export const helper = 1;
