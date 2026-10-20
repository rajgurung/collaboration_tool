<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ name | escape }},</p>
  <p>Someone typed a wrong app password 5 times while trying to see logins in {{ org | escape }}, so your account is locked and signed out everywhere. Claude was disconnected.</p>
  <p>To unlock it, reset your password:</p>
  <p><a href="{{ forgot_url | escape }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">Reset your password</a></p>
  <p style="color: #666; font-size: 13px;">If it wasn't you, someone may be using your phone or computer.</p>
</body>
</html>
