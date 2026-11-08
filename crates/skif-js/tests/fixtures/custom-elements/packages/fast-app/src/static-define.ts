import { FASTElement } from '@microsoft/fast-element';

export class StaticDefined extends FASTElement {}

FASTElement.define(StaticDefined, { name: 'x-static-defined' });
