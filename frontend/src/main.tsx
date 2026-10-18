import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ready } from './lib/i18n'
import './styles.css'

const root = document.getElementById('root')
if (!root) throw new Error('#root is missing from index.html')

// The reader's words before anything is drawn: drawn first, the page would
// show another language for as long as theirs took to arrive. Waited for
// here, not with a top-level `await`, which split the first bundle into a
// score of small files to keep the order modules run in.
void ready().then(() =>
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  ),
)
