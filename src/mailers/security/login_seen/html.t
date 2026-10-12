<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ name | escape }},</p>
  <p><strong>{{ who | escape }}</strong> {{ verb | escape }} {{ what | escape }} for <strong>{{ title | escape }}</strong> in {{ org | escape }}, at {{ at | escape }}.</p>
  <p>You get this email each time someone sees or copies a login password. If you did not expect it, look at Login activity and change the password on its own website.</p>
  <p><a href="{{ activity_url | escape }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">Open Login activity</a></p>
</body>
</html>
