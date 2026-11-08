import { useState } from "@tsrx/react";
import Button from "./Button";
import "./App.styl";

export component App() {
  const [count, setCount] = useState(0);
  <Button onClick={() => setCount(count + 1)}>{count}</Button>
}

export default App;
