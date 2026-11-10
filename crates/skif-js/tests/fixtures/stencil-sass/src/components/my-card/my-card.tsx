import { Component, h } from "@stencil/core";

@Component({
  tag: "my-card",
  styleUrl: "my-card.scss",
  shadow: true,
})
export class MyCard {
  render() {
    return <div class="card" />;
  }
}
