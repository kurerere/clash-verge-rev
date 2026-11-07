import { Visibility, VisibilityOff } from '@mui/icons-material'
import {
  IconButton,
  InputAdornment,
  TextField,
  Typography,
} from '@mui/material'
import { useState, useSyncExternalStore } from 'react'
import { useTranslation } from 'react-i18next'

import { BaseDialog } from '@/components/base'
import {
  getSubscriptionPasswordRequest,
  resolveSubscriptionPassword,
  subscribeSubscriptionPassword,
} from '@/services/subscription-password'

function PasswordDialog({
  name,
  incorrect,
}: {
  name: string
  incorrect: boolean
}) {
  const { t } = useTranslation()
  const [password, setPassword] = useState('')
  const [visible, setVisible] = useState(false)
  const submit = () => {
    if (password) resolveSubscriptionPassword(password)
  }
  const cancel = () => resolveSubscriptionPassword(null)

  return (
    <BaseDialog
      open
      title={t('profiles.modals.subscriptionPassword.title')}
      okBtn={t('shared.actions.confirm')}
      cancelBtn={t('shared.actions.cancel')}
      okDisabled={!password}
      onOk={submit}
      onCancel={cancel}
      onClose={cancel}
      contentSx={{ width: { xs: 320, sm: 420 } }}
    >
      <Typography variant="body2">
        {t('profiles.modals.subscriptionPassword.message', { name })}
      </Typography>
      <TextField
        autoFocus
        fullWidth
        margin="normal"
        type={visible ? 'text' : 'password'}
        label={t('profiles.modals.profileForm.fields.subscriptionPassword')}
        value={password}
        onChange={(event) => setPassword(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') submit()
        }}
        slotProps={{
          input: {
            endAdornment: (
              <InputAdornment position="end">
                <IconButton
                  aria-label={t(
                    'profiles.modals.profileForm.fields.subscriptionPassword',
                  )}
                  aria-pressed={visible}
                  onClick={() => setVisible(!visible)}
                  edge="end"
                >
                  {visible ? <VisibilityOff /> : <Visibility />}
                </IconButton>
              </InputAdornment>
            ),
          },
        }}
      />
      {incorrect && (
        <Typography color="error" variant="body2">
          {t('profiles.modals.subscriptionPassword.incorrect')}
        </Typography>
      )}
    </BaseDialog>
  )
}

export function SubscriptionPasswordDialog() {
  const request = useSyncExternalStore(
    subscribeSubscriptionPassword,
    getSubscriptionPasswordRequest,
  )
  return request ? <PasswordDialog key={request.id} {...request} /> : null
}
