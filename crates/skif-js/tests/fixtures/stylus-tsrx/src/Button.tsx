export default function Button(props: { onClick: () => void; children?: unknown }) {
  return <button onClick={props.onClick}>{String(props.children)}</button>;
}
