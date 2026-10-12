<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ name | escape }},</p>
  <p><strong>{{ who | escape }}</strong> typed a wrong app password 5 times while trying to see logins in {{ org | escape }}, so their account is locked and signed out everywhere. The last try was at {{ at | escape }}.</p>
  <p>They can unlock it by resetting their password with "Forgot password" on the sign-in page. If it was not them, someone else may be using their phone or computer. Talk to them before they reset it.</p>
  <p><a href="{{ activity_url | escape }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">Open Login activity</a></p>
</body>
</html>
