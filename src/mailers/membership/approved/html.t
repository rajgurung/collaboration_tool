<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ username | escape }},</p>
  <p>You're in {{ org_name | escape }} on Collaboration Tool. People there see you as {{ username | escape }}.</p>
  <p>To open it, sign in and choose {{ org_name | escape }} from the organisation menu at the top.</p>
  <p><a href="{{ dashboard_url | escape }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">Open Collaboration Tool</a></p>
  <p style="color: #666; font-size: 13px;">Or paste this address into your browser: {{ dashboard_url | escape }}</p>
  <p style="color: #666; font-size: 13px;">If you're already signed in, {{ org_name | escape }} shows in your organisations next time a page loads.</p>
</body>
</html>
