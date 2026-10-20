<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ name | escape }},</p>
  <p>The password for your Collaboration Tool account ({{ email | escape }}) was just changed. Every device that was signed in has been signed out.</p>
  {% if claude_disconnected %}<p>Claude was disconnected too. If you use it, connect it again from More, then Connect Claude.</p>{% endif %}
  <p>If you did this, there is nothing else to do.</p>
  <p>If you did not, someone else can read your email. Secure your email account first, then reset your password again:</p>
  <p><a href="{{ forgot_url | escape }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">Reset your password</a></p>
  <p style="color: #666; font-size: 13px;">Then tell an owner of each organisation you belong to.</p>
</body>
</html>
