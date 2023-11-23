use actix_web::{get, HttpResponse, Responder, web::{self, ReqData}, post};
use reqwest::Client;
use serde::Deserialize;
use crate::{ AppState,helpers::firebase};
use serde_json::json;
use entity::user::UserEntity;
use schema::authmgmt::{
    req::Req,
    resp::Resp,
};
use schema::usermgmt::{
    user::CreateUserRequest,
    user::CreateUserResponse,
};
use utils::read_file::read_config;
use utils::constants::{
    ADMIN_GROUP,
    CUSTOMER_GROUP,
    DRIVER_GROUP,
};


#[derive(Deserialize, Debug)]
struct  FilterOptions{}

#[get("/whoami")]
async fn whoami(
    firebase_user: ReqData<firebase::FirebaseUser>,
    data: web::Data<AppState>,
) -> impl Responder {
    let user_id = firebase_user.user_id.clone();
    let user_group = firebase_user.user_group.clone();
    let db_user_id = firebase_user.db_user_id.parse::<i32>().unwrap();

    let query_result = sqlx::query_as!(
        UserEntity,
        "SELECT user_id, email, firebase_uid, first_name, last_name, user_group, phone_number FROM users WHERE firebase_uid = $1 AND user_group = $2 AND user_id = $3",
        user_id,
        user_group,
        db_user_id,
    )
    .fetch_all(&data.db)
    .await;

    if query_result.is_err() {
        let message = "Something bad happened while fetching all note items";
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error","message": message}));
    }

    let users = query_result.unwrap();
    if users.len() == 0 {
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error","message": "User not found"}));
    }
    println!("users: {:?}", users);
    let json_response = serde_json::json!({
        "status": "success",
        "results": users[0],
    });
    HttpResponse::Ok().json(json_response)
}

#[get("/users")]
async fn get_all_user( 
    _: web::Query<FilterOptions>,
    data: web::Data<AppState>,
) -> impl Responder {

    let query_result = sqlx::query_as!(
        UserEntity,
        "SELECT user_id, email, firebase_uid, first_name, last_name, user_group, phone_number FROM users",
    )
    .fetch_all(&data.db)
    .await;

    if query_result.is_err() {
        let message = "Something bad happened while fetching all note items";
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error","message": message}));
    }

    let users = query_result.unwrap();
    let json_response = serde_json::json!({
        "status": "success",
        "results": users.len(),
        "users": users
    });
    HttpResponse::Ok().json(json_response)
}


#[post("/create")]
async fn create_user(
    body: web::Json<CreateUserRequest>,
    data: web::Data<AppState>,
    firebase_user: ReqData<firebase::FirebaseUser>,
) -> impl Responder {
    let user_group = firebase_user.user_group.clone();

    if user_group != ADMIN_GROUP {
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error","message": "You are not allowed to create user"}));
    }
    let req = body.into_inner();
    println!("req: {:?}", req);
    let user = CreateUserRequest {
        email: req.email,
        first_name: req.first_name,
        last_name: req.last_name,
        user_group: req.user_group,
        password: req.password,
        phone_number: req.phone_number,
        vehicle_type_id: req.vehicle_type_id,
    };

    if user.user_group != ADMIN_GROUP && user.user_group != CUSTOMER_GROUP && user.user_group != DRIVER_GROUP {
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error","message": "Invalid user group"}));
    }

    if user.user_group == ADMIN_GROUP {
        return HttpResponse::InternalServerError()
            .json(json!({"status": "error","message": "You are not allowed to create admin user"}));
    }

    let firebase_user  = Req{
        email: user.email,
        password: user.password,
    };
    let path = String::from("/common-configmap/url_auth_service");
    let ip_service = match read_config(path) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            return HttpResponse::InternalServerError().json(e.to_string());
        }
    };
    let endpoint = format!("http://{}:8080/authmgmt/create-firebase-user", ip_service);

    // Start a transaction
    let tx =  data.db.begin().await;
    if tx.is_err() {
        return HttpResponse::InternalServerError().json(tx.err().unwrap().to_string());
   }
    let res = match Client::new()
    .post(&endpoint)
    .json(&firebase_user)
    .send()
    .await {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Make http req have an error: {}", e);
            return HttpResponse::InternalServerError().json(e.to_string());
        }
    };
    let response_text: String = match res.text().await  {
            Ok(body) => body,
            Err(e) => {
                eprintln!("Error response_text: {}", e);
                return HttpResponse::InternalServerError().json(e.to_string());
            }
    };
    // Attempt to parse the trimmed response as JSON
    let body: Resp = match  serde_json::from_str(response_text.trim()) {
        Ok(body) => body,
        Err(e) => {
            eprintln!("Error reading body: {}", e);
            return HttpResponse::InternalServerError().json(e.to_string());
        }
    };

    // INSERT data into users table
    let query_result = sqlx::query!(
        "INSERT INTO users (last_name, first_name, email, user_group, firebase_uid, phone_number) VALUES ($1, $2, $3, $4, $5, $6) RETURNING user_id",
        user.first_name,user.last_name, body.email, user.user_group, body.uid, user.phone_number,
    )
    .fetch_one(&data.db)
    .await;

    if query_result.is_err() {
        println!("--1--");
        let e = query_result.err().expect("No error INSERT data into users table");
        return HttpResponse::InternalServerError().json(e.to_string());
    }
    let r = query_result.expect("No error when get user_id");
    let user_id = r.user_id;

    // Create CUSTOMER_GROUP
    if user.user_group == CUSTOMER_GROUP {
        let query_result = sqlx::query!(
            "INSERT INTO customers (user_id) VALUES ($1)",
            user_id,
        )
        .execute(&data.db)
        .await;
        if query_result.is_err() {
            println!("--2--");
            let e = query_result.err().expect("No error INSERT data into customers table");
            return HttpResponse::InternalServerError().json(e.to_string());
        }
    }

    // Create DRIVER_GROUP
    if user.user_group == DRIVER_GROUP {
        let query_result = sqlx::query!(
            "INSERT INTO drivers (user_id, vehicle_type_id, status) VALUES ($1, $2, $3)",
            user_id,
            user.vehicle_type_id,
            "OFFLINE"
        )
        .execute(&data.db)
        .await;
        if query_result.is_err() {
            println!("--3--");
            let e = query_result.err().expect("No error INSERT data into drivers table");
            return HttpResponse::InternalServerError().json(e.to_string());
        }
    }

    // commit the transaction
    let commit = tx.unwrap().commit().await;
    if commit.is_err() {
        println!("--4--");
        let e = commit.err().expect("No error commit the transaction");
        return HttpResponse::InternalServerError().json(e.to_string());
    }
    let user_resp = CreateUserResponse {
        email: body.email,
        first_name: user.first_name,
        last_name: user.last_name,
        user_group: user.user_group,
        user_id: user_id,
        phone_number: user.phone_number,
    };
    println!("user_resp: {:?}", user_resp);
    HttpResponse::Ok().json(user_resp)
}

pub fn config(conf: &mut web::ServiceConfig) {
    conf.service(whoami)
        .service(get_all_user)
        .service(create_user);
}