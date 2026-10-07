use loco_rs::prelude::*;

use crate::models::{_entities::users, users::RegisterParams};

pub struct UserCreate;
#[async_trait]
impl Task for UserCreate {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "user:create".to_string(),
            detail: "Create a new user with email, username (name) and password.\nUsage:\ncargo run task user:create email:user@example.com name:\"john\" password:\"securepassword\"".to_string(),
        }
    }
    async fn run(&self, app_context: &AppContext, vars: &task::Vars) -> Result<()> {
        let email = vars
            .cli_arg("email")
            .map_err(|_| Error::string("email is mandatory"))?;
        let name = vars
            .cli_arg("name")
            .map_err(|_| Error::string("name is mandatory"))?;
        let password = vars
            .cli_arg("password")
            .map_err(|_| Error::string("password is mandatory"))?;

        let register_params = RegisterParams {
            email: email.to_owned(),
            password: password.to_owned(),
            name: name.to_owned(),
        };

        // Create user with password using the same logic as register controller
        let res = users::Model::create_with_password(&app_context.db, &register_params).await;

        let user = match res {
            Ok(user) => {
                tracing::info!(
                    message = "User created successfully",
                    user_email = &register_params.email,
                    user_pid = user.pid.to_string(),
                    "user created via task"
                );
                user
            }
            Err(err) => {
                tracing::error!(
                    message = err.to_string(),
                    user_email = &register_params.email,
                    "could not create user via task"
                );
                return Err(Error::string(&format!("Failed to create user. err: {err}")));
            }
        };

        tracing::info!(
            message = "User creation task completed successfully",
            user_email = &register_params.email,
            user_pid = user.pid.to_string(),
            "user creation task finished"
        );

        println!("✅ User created successfully!");
        println!("   Email: {}", user.email);
        println!("   Name: {}", user.name);
        println!("   PID: {}", user.pid);

        Ok(())
    }
}
