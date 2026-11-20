import { lazy, Suspense } from 'react'
import { Navigate, Route, Routes, useLocation } from 'react-router'
import { ErrorBoundary } from './components/ErrorBoundary'
import Landing from './pages/landing/Landing'

const AuthPage = lazy(() => import('./pages/auth/AuthPage'))
const ConsoleShell = lazy(() => import('./pages/console/ConsoleShell'))
const BotsHome = lazy(() => import('./pages/console/BotsHome'))
const BotPage = lazy(() => import('./pages/console/bot/BotPage'))
const UsagePage = lazy(() => import('./pages/console/UsagePage'))
const KeysPage = lazy(() => import('./pages/console/KeysPage'))
const AccountPage = lazy(() => import('./pages/console/AccountPage'))
const AboutPage = lazy(() => import('./pages/AboutPage'))
const AdminOverview = lazy(() => import('./pages/console/admin/AdminOverview'))
const AdminUsers = lazy(() => import('./pages/console/admin/AdminUsers'))
const AdminUserDetail = lazy(() => import('./pages/console/admin/AdminUserDetail'))
const AdminInvites = lazy(() => import('./pages/console/admin/AdminInvites'))
const AdminSystem = lazy(() => import('./pages/console/admin/AdminSystem'))

export default function App() {
  const location = useLocation()
  return (
    <ErrorBoundary variant="screen" resetKey={location.pathname}>
      <Suspense fallback={<div className="min-h-dvh bg-paper" />}>
        <Routes>
          <Route path="/" element={<Landing />} />
          <Route path="/login" element={<AuthPage mode="login" />} />
          <Route path="/register" element={<AuthPage mode="register" />} />
          <Route path="/reset" element={<AuthPage mode="reset" />} />
          <Route path="/about" element={<AboutPage />} />
          <Route path="/app" element={<ConsoleShell />}>
            <Route index element={<BotsHome />} />
            <Route path="bots/:botId/:tab?" element={<BotPage />} />
            <Route path="usage" element={<UsagePage />} />
            <Route path="keys" element={<KeysPage />} />
            <Route path="account" element={<AccountPage />} />
            <Route path="admin" element={<AdminOverview />} />
            <Route path="admin/users" element={<AdminUsers />} />
            <Route path="admin/users/:userId/:section?" element={<AdminUserDetail />} />
            <Route path="admin/invites" element={<AdminInvites />} />
            <Route path="admin/system" element={<AdminSystem />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </Suspense>
    </ErrorBoundary>
  )
}
