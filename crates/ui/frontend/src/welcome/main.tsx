import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
// The Settings tokens and component classes (`btn`, `CheckLine`), so both windows match.
import '../settings/App.css'

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
